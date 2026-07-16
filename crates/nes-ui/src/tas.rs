use std::{
    collections::{BTreeMap, VecDeque},
    error::Error,
    fmt, fs, io,
    path::Path,
};

// FCEUX's TAS Editor informed this module's high-level workflow, but this is an
// independent Rust implementation and contains no FCEUX code. See
// THIRD_PARTY_NOTICES.md.

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use sha2::{Digest, Sha256};

pub const FORMAT_VERSION: u32 = 1;
pub const DEFAULT_CHECKPOINT_INTERVAL: usize = 300;
pub const EMULATOR_NAME: &str = "CrabNes";
const LEGACY_EMULATOR_NAME: &str = "MyOwnNesEmulator";
pub const EMULATOR_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TasFrame {
    pub player1: u8,
    pub player2: u8,
}

impl TasFrame {
    pub fn with_held_input(self, held: Self) -> Self {
        Self {
            player1: self.player1 | held.player1,
            player2: self.player2 | held.player2,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Region {
    Ntsc,
}

impl Region {
    fn as_text(self) -> &'static str {
        match self {
            Self::Ntsc => "NTSC",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TasStartType {
    PowerOn,
    Reset,
    SaveState,
}

impl TasStartType {
    fn as_text(self) -> &'static str {
        match self {
            Self::PowerOn => "POWER_ON",
            Self::Reset => "RESET",
            Self::SaveState => "SAVE_STATE",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TasMarker {
    pub frame: usize,
    pub label: String,
}

#[derive(Clone, Debug)]
pub struct TasMovie {
    pub format_version: u32,
    pub rom_sha256: String,
    pub emulator_version: String,
    pub region: Region,
    pub start_type: TasStartType,
    pub starting_state: Option<Vec<u8>>,
    pub rerecord_count: u64,
    pub author: Option<String>,
    pub description: Option<String>,
    pub frames: Vec<TasFrame>,
    pub markers: Vec<TasMarker>,
    /// Hashes of the machine state immediately before the keyed frame.
    pub state_checksums: BTreeMap<usize, String>,
}

impl TasMovie {
    pub fn new(
        rom_sha256: String,
        start_type: TasStartType,
        starting_state: Option<Vec<u8>>,
    ) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            rom_sha256,
            emulator_version: EMULATOR_VERSION.to_owned(),
            region: Region::Ntsc,
            start_type,
            starting_state,
            rerecord_count: 0,
            author: None,
            description: None,
            frames: Vec::new(),
            markers: Vec::new(),
            state_checksums: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TasMode {
    Inactive,
    Recording,
    Playback,
    Paused,
    ReadOnly,
}

#[derive(Clone)]
pub struct TasCheckpoint {
    pub frame: usize,
    pub state: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckpointReconciliation {
    /// The live and independently replayed states agree, so only the stored
    /// checksum belongs to an older state layout or emulator revision.
    RefreshChecksum,
    /// Independent replay produced the state named by the movie checksum, so
    /// playback can safely restore that state and continue.
    RestoreReplay,
    /// Neither current execution nor replay can reproduce the movie checksum.
    Unrecoverable,
}

pub fn reconcile_checkpoint(
    expected: &str,
    live_state: &[u8],
    replayed_state: &[u8],
) -> CheckpointReconciliation {
    if live_state == replayed_state {
        CheckpointReconciliation::RefreshChecksum
    } else if sha256_hex(replayed_state) == expected {
        CheckpointReconciliation::RestoreReplay
    } else {
        CheckpointReconciliation::Unrecoverable
    }
}

pub struct TasRecorder;
pub struct TasPlayback;
pub struct TasEditor;
pub struct TasSerializer;
pub struct TasDeserializer;

impl TasRecorder {
    fn record(movie: &mut TasMovie, cursor: usize, input: TasFrame) {
        if cursor < movie.frames.len() {
            movie.frames[cursor] = input;
            movie.frames.truncate(cursor + 1);
            movie.rerecord_count = movie.rerecord_count.saturating_add(1);
            movie.state_checksums.retain(|frame, _| *frame <= cursor);
        } else {
            movie.frames.push(input);
        }
    }
}

impl TasPlayback {
    fn input(movie: &TasMovie, cursor: usize) -> Option<TasFrame> {
        movie.frames.get(cursor).copied()
    }
}

impl TasEditor {
    pub fn set_frame(movie: &mut TasMovie, frame: usize, input: TasFrame) -> bool {
        let Some(current) = movie.frames.get_mut(frame) else {
            return false;
        };
        if *current == input {
            return false;
        }
        *current = input;
        Self::changed_from(movie, frame);
        true
    }

    pub fn insert(movie: &mut TasMovie, frame: usize, inputs: &[TasFrame]) -> bool {
        if inputs.is_empty() || frame > movie.frames.len() {
            return false;
        }
        movie.frames.splice(frame..frame, inputs.iter().copied());
        for marker in &mut movie.markers {
            if marker.frame >= frame {
                marker.frame = marker.frame.saturating_add(inputs.len());
            }
        }
        Self::changed_from(movie, frame);
        true
    }

    pub fn delete(movie: &mut TasMovie, start: usize, end: usize) -> bool {
        if movie.frames.is_empty() {
            return false;
        }
        let start = start.min(movie.frames.len() - 1);
        let end = end.min(movie.frames.len() - 1);
        let (start, end) = ordered(start, end);
        let removed = end - start + 1;
        movie.frames.drain(start..=end);
        movie.markers.retain_mut(|marker| {
            if (start..=end).contains(&marker.frame) {
                false
            } else {
                if marker.frame > end {
                    marker.frame -= removed;
                }
                true
            }
        });
        Self::changed_from(movie, start);
        true
    }

    pub fn clear(movie: &mut TasMovie, start: usize, end: usize) -> bool {
        Self::fill(movie, start, end, TasFrame::default())
    }

    pub fn fill(movie: &mut TasMovie, start: usize, end: usize, input: TasFrame) -> bool {
        if movie.frames.is_empty() {
            return false;
        }
        let (start, end) = ordered(
            start.min(movie.frames.len() - 1),
            end.min(movie.frames.len() - 1),
        );
        movie.frames[start..=end].fill(input);
        Self::changed_from(movie, start);
        true
    }

    pub fn paste_overwrite(movie: &mut TasMovie, frame: usize, inputs: &[TasFrame]) -> bool {
        if inputs.is_empty() || frame > movie.frames.len() {
            return false;
        }
        if movie.frames.len() < frame + inputs.len() {
            movie
                .frames
                .resize(frame + inputs.len(), TasFrame::default());
        }
        movie.frames[frame..frame + inputs.len()].copy_from_slice(inputs);
        Self::changed_from(movie, frame);
        true
    }

    fn changed_from(movie: &mut TasMovie, frame: usize) {
        movie.rerecord_count = movie.rerecord_count.saturating_add(1);
        movie
            .state_checksums
            .retain(|checkpoint, _| *checkpoint <= frame);
    }
}

pub struct TasManager {
    pub mode: TasMode,
    resume_mode: TasMode,
    pub movie: Option<TasMovie>,
    /// Index of the next movie frame to execute.
    pub cursor: usize,
    pub selected_frame: usize,
    pub range_end_frame: usize,
    pub checkpoints: Vec<TasCheckpoint>,
    pub clipboard: Vec<TasFrame>,
    pub marker_label: String,
    pub checkpoint_interval: usize,
    pub last_desync: Option<String>,
    logs: VecDeque<String>,
}

impl Default for TasManager {
    fn default() -> Self {
        Self {
            mode: TasMode::Inactive,
            resume_mode: TasMode::Inactive,
            movie: None,
            cursor: 0,
            selected_frame: 0,
            range_end_frame: 0,
            checkpoints: Vec::new(),
            clipboard: Vec::new(),
            marker_label: String::new(),
            checkpoint_interval: DEFAULT_CHECKPOINT_INTERVAL,
            last_desync: None,
            logs: VecDeque::new(),
        }
    }
}

impl TasManager {
    pub fn new_movie(&mut self, mut movie: TasMovie, initial_state: Vec<u8>) {
        movie.state_checksums.insert(0, sha256_hex(&initial_state));
        self.movie = Some(movie);
        self.mode = TasMode::Recording;
        self.resume_mode = TasMode::Recording;
        self.cursor = 0;
        self.selected_frame = 0;
        self.range_end_frame = 0;
        self.checkpoints = vec![TasCheckpoint {
            frame: 0,
            state: initial_state,
        }];
        self.clipboard.clear();
        self.last_desync = None;
        self.log("recording started at frame 0");
    }

    pub fn install_movie(&mut self, movie: TasMovie) {
        self.movie = Some(movie);
        self.mode = TasMode::Inactive;
        self.resume_mode = TasMode::Inactive;
        self.cursor = 0;
        self.selected_frame = 0;
        self.range_end_frame = 0;
        self.checkpoints.clear();
        self.clipboard.clear();
        self.last_desync = None;
        self.log("movie loaded");
    }

    pub fn start_playback(&mut self, read_only: bool) -> bool {
        if self.movie.is_none() {
            return false;
        }
        self.cursor = 0;
        self.mode = if read_only {
            TasMode::ReadOnly
        } else {
            TasMode::Playback
        };
        self.resume_mode = self.mode;
        self.last_desync = None;
        self.log(if read_only {
            "read-only playback started"
        } else {
            "playback started"
        });
        true
    }

    pub fn stop(&mut self) {
        if self.mode != TasMode::Inactive {
            self.log("TAS stopped");
        }
        self.mode = TasMode::Inactive;
        self.resume_mode = TasMode::Inactive;
    }

    pub fn pause(&mut self) {
        if matches!(
            self.mode,
            TasMode::Recording | TasMode::Playback | TasMode::ReadOnly
        ) {
            self.resume_mode = self.mode;
            self.mode = TasMode::Paused;
            self.log("TAS paused");
        }
    }

    pub fn resume(&mut self) {
        if self.mode == TasMode::Paused {
            self.mode = self.resume_mode;
            self.log("TAS resumed");
        }
    }

    pub fn resume_recording(&mut self) -> bool {
        if self.read_only()
            || self
                .movie
                .as_ref()
                .is_none_or(|movie| self.cursor > movie.frames.len())
        {
            return false;
        }
        self.mode = TasMode::Recording;
        self.resume_mode = TasMode::Recording;
        self.invalidate_after(self.cursor);
        self.log(format!("rerecording resumed at frame {}", self.cursor));
        true
    }

    /// Put the movie in the correct mode for one frame of stepping.
    ///
    /// Existing rows are previewed without modification. At the editable end
    /// row, frame advance automatically continues recording so blank or live
    /// input creates a new movie row instead of silently running outside TAS.
    pub fn prepare_frame_advance(&mut self) -> bool {
        let Some(frame_count) = self.movie.as_ref().map(|movie| movie.frames.len()) else {
            return true;
        };
        if self.cursor < frame_count {
            if self.mode == TasMode::Inactive {
                self.set_cursor_paused_for_preview(self.cursor);
            }
            return true;
        }
        if self.read_only() {
            return false;
        }
        if !self.recording_context() && !self.resume_recording() {
            return false;
        }
        true
    }

    pub fn input_for_frame(&mut self, live: TasFrame) -> Option<TasFrame> {
        let effective_mode = if self.mode == TasMode::Paused {
            self.resume_mode
        } else {
            self.mode
        };
        let input = match effective_mode {
            TasMode::Inactive => Some(live),
            TasMode::Recording => {
                let movie = self.movie.as_mut()?;
                let rerecords = movie.rerecord_count;
                TasRecorder::record(movie, self.cursor, live);
                if movie.rerecord_count != rerecords {
                    self.log(format!("rerecord count increased at frame {}", self.cursor));
                }
                Some(live)
            }
            TasMode::Playback | TasMode::ReadOnly => {
                TasPlayback::input(self.movie.as_ref()?, self.cursor)
            }
            TasMode::Paused => unreachable!(),
        };
        if input.is_some() && effective_mode != TasMode::Inactive {
            self.cursor += 1;
            if self.cursor.is_multiple_of(60) {
                self.log(format!("current TAS frame {}", self.cursor));
            }
        }
        input
    }

    pub fn read_only(&self) -> bool {
        self.mode == TasMode::ReadOnly
            || (self.mode == TasMode::Paused && self.resume_mode == TasMode::ReadOnly)
    }

    pub fn editable(&self) -> bool {
        self.movie.is_some() && !self.read_only()
    }

    /// Returns true while live input is being recorded, including when that
    /// recording is paused for frame stepping or rewind.
    pub fn recording_context(&self) -> bool {
        self.mode == TasMode::Recording
            || (self.mode == TasMode::Paused && self.resume_mode == TasMode::Recording)
    }

    /// Creates a rerecord branch at `frame` after the machine has been rewound.
    ///
    /// The restored machine state is immediately before this frame's input, so
    /// that frame and every later frame must be discarded. Playback and
    /// read-only rewind deliberately leave the movie untouched.
    pub fn truncate_recording_at(&mut self, frame: usize) -> usize {
        if !self.recording_context() || self.read_only() {
            return 0;
        }
        let Some(movie) = &mut self.movie else {
            return 0;
        };
        let frame = frame.min(movie.frames.len());
        let removed = movie.frames.len().saturating_sub(frame);
        if removed == 0 {
            self.cursor = frame;
            self.selected_frame = frame;
            self.range_end_frame = frame;
            self.mode = TasMode::Paused;
            self.resume_mode = TasMode::Recording;
            return 0;
        }

        movie.frames.truncate(frame);
        movie.markers.retain(|marker| marker.frame < frame);
        movie
            .state_checksums
            .retain(|checkpoint, _| *checkpoint <= frame);
        movie.rerecord_count = movie.rerecord_count.saturating_add(1);
        self.checkpoints.retain(|point| point.frame <= frame);
        self.cursor = frame;
        self.selected_frame = frame;
        self.range_end_frame = frame;
        self.mode = TasMode::Paused;
        self.resume_mode = TasMode::Recording;
        self.log(format!(
            "rewound to frame {frame}; removed {removed} recorded input frame(s)"
        ));
        removed
    }

    pub fn set_cursor_paused(&mut self, frame: usize) {
        self.cursor = frame;
        if self.mode != TasMode::Paused {
            self.resume_mode = match self.mode {
                TasMode::Inactive => TasMode::Playback,
                other => other,
            };
        }
        self.mode = TasMode::Paused;
        self.selected_frame = frame.min(self.movie.as_ref().map_or(0, |movie| movie.frames.len()));
        self.range_end_frame = self.selected_frame;
        self.log(format!("seek paused at frame {frame}"));
    }

    /// Pause at a movie frame for deterministic preview playback.
    ///
    /// Seeking out of a live recording must not leave the paused manager in
    /// recording mode: the next frame advance would otherwise overwrite the
    /// edited movie input with the current host controller state. Rerecording
    /// remains an explicit action through `resume_recording`.
    pub fn set_cursor_paused_for_preview(&mut self, frame: usize) {
        let preview_mode = if self.read_only() {
            TasMode::ReadOnly
        } else {
            TasMode::Playback
        };
        self.cursor = frame;
        self.resume_mode = preview_mode;
        self.mode = TasMode::Paused;
        self.selected_frame = frame.min(self.movie.as_ref().map_or(0, |movie| movie.frames.len()));
        self.range_end_frame = self.selected_frame;
        self.log(format!("preview paused at frame {frame}"));
    }

    /// Record or validate a periodic machine state.
    ///
    /// Returns `true` for every checksum mismatch, even when an older mismatch
    /// is still displayed. Callers must use this return value rather than the
    /// transition of `last_desync` so one stale checkpoint cannot suppress all
    /// later reconciliation attempts.
    pub fn maybe_checkpoint(&mut self, frame: usize, state: Vec<u8>) -> bool {
        if !frame.is_multiple_of(self.checkpoint_interval.max(1)) {
            return false;
        }
        let checksum = sha256_hex(&state);
        let mut mismatched = false;
        let effective_mode = if self.mode == TasMode::Paused {
            self.resume_mode
        } else {
            self.mode
        };
        if let Some(movie) = &mut self.movie {
            match effective_mode {
                TasMode::Recording => {
                    movie.state_checksums.insert(frame, checksum);
                }
                TasMode::Inactive | TasMode::Playback | TasMode::ReadOnly => {
                    if let Some(expected) = movie.state_checksums.get(&frame)
                        && expected != &checksum
                    {
                        mismatched = true;
                        self.last_desync = Some(format!(
                            "desync at frame {frame}: expected {expected}, got {checksum}"
                        ));
                    }
                }
                _ => {}
            }
        }
        if !self.checkpoints.iter().any(|point| point.frame == frame) {
            self.checkpoints.push(TasCheckpoint { frame, state });
            self.checkpoints.sort_by_key(|point| point.frame);
            self.log(format!("checkpoint created at frame {frame}"));
        }
        if mismatched && let Some(desync) = self.last_desync.clone() {
            self.log(desync);
        }
        mismatched
    }

    pub fn checkpoint_at_or_before(&self, target: usize) -> Option<TasCheckpoint> {
        self.checkpoints
            .iter()
            .rev()
            .find(|point| point.frame <= target)
            .cloned()
    }

    pub fn repair_checkpoint_checksum(&mut self, frame: usize, state: &[u8]) {
        if let Some(movie) = &mut self.movie {
            movie.state_checksums.insert(frame, sha256_hex(state));
        }
        self.last_desync = None;
        self.log(format!(
            "repaired stale checkpoint metadata at frame {frame} after deterministic replay"
        ));
    }

    pub fn accept_resynchronized_checkpoint(&mut self, frame: usize, state: Vec<u8>) {
        if let Some(checkpoint) = self
            .checkpoints
            .iter_mut()
            .find(|checkpoint| checkpoint.frame == frame)
        {
            checkpoint.state = state;
        } else {
            self.checkpoints.push(TasCheckpoint { frame, state });
            self.checkpoints.sort_by_key(|checkpoint| checkpoint.frame);
        }
        self.last_desync = None;
        self.log(format!(
            "resynchronized machine state at frame {frame} from deterministic replay"
        ));
    }

    pub fn invalidate_after(&mut self, frame: usize) {
        self.checkpoints.retain(|point| point.frame <= frame);
        if let Some(movie) = &mut self.movie {
            movie.state_checksums.retain(|point, _| *point <= frame);
        }
        self.log(format!(
            "future checkpoints invalidated after frame {frame}"
        ));
    }

    pub fn copy_selection(&mut self) -> bool {
        let Some(movie) = &self.movie else {
            return false;
        };
        if movie.frames.is_empty() {
            return false;
        }
        let (start, end) = ordered(
            self.selected_frame.min(movie.frames.len() - 1),
            self.range_end_frame.min(movie.frames.len() - 1),
        );
        self.clipboard = movie.frames[start..=end].to_vec();
        self.log(format!("copied frames {start} through {end}"));
        true
    }

    pub fn paste_selection(&mut self, insert: bool) -> bool {
        if !self.editable() || self.clipboard.is_empty() {
            return false;
        }
        let frame = self.selected_frame;
        let clipboard = self.clipboard.clone();
        let movie = self.movie.as_mut().unwrap();
        let changed = if insert {
            TasEditor::insert(movie, frame, &clipboard)
        } else {
            TasEditor::paste_overwrite(movie, frame, &clipboard)
        };
        if changed {
            self.invalidate_after(frame);
            self.log(format!(
                "{} {} frame(s) at {frame}",
                if insert { "inserted" } else { "pasted" },
                clipboard.len()
            ));
        }
        changed
    }

    pub fn add_marker(&mut self, frame: usize, label: String) -> bool {
        if !self.editable() || label.trim().is_empty() {
            return false;
        }
        let movie = self.movie.as_mut().unwrap();
        movie.markers.retain(|marker| marker.frame != frame);
        movie.markers.push(TasMarker {
            frame,
            label: label.trim().to_owned(),
        });
        movie.markers.sort_by_key(|marker| marker.frame);
        self.log(format!("marker added at frame {frame}"));
        true
    }

    pub fn remove_marker(&mut self, frame: usize) -> bool {
        if !self.editable() {
            return false;
        }
        let movie = self.movie.as_mut().unwrap();
        let before = movie.markers.len();
        movie.markers.retain(|marker| marker.frame != frame);
        let changed = before != movie.markers.len();
        if changed {
            self.log(format!("marker removed at frame {frame}"));
        }
        changed
    }

    pub fn logs(&self) -> impl Iterator<Item = &str> {
        self.logs.iter().map(String::as_str)
    }

    pub fn log(&mut self, message: impl Into<String>) {
        self.logs.push_back(message.into());
        while self.logs.len() > 200 {
            self.logs.pop_front();
        }
    }
}

#[derive(Debug)]
pub enum TasFormatError {
    Io(io::Error),
    Invalid(String),
    UnsupportedVersion(u32),
    WrongRom { expected: String, actual: String },
}

impl fmt::Display for TasFormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(f),
            Self::Invalid(error) => write!(f, "invalid TAS file: {error}"),
            Self::UnsupportedVersion(version) => {
                write!(f, "TAS format version {version} is not supported")
            }
            Self::WrongRom { expected, actual } => write!(
                f,
                "TAS belongs to another ROM (expected {expected}, file contains {actual})"
            ),
        }
    }
}

impl Error for TasFormatError {}

impl From<io::Error> for TasFormatError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

pub struct LoadedTas {
    pub movie: TasMovie,
    pub warnings: Vec<String>,
}

impl TasSerializer {
    pub fn serialize(movie: &TasMovie) -> Result<String, TasFormatError> {
        validate_movie(movie)?;
        let mut text = String::new();
        push_line(&mut text, "TAS_FORMAT", &movie.format_version.to_string());
        push_line(&mut text, "EMULATOR", EMULATOR_NAME);
        push_line(&mut text, "EMULATOR_VERSION", &movie.emulator_version);
        push_line(&mut text, "ROM_SHA256", &movie.rom_sha256);
        push_line(&mut text, "REGION", movie.region.as_text());
        push_line(&mut text, "START_TYPE", movie.start_type.as_text());
        push_line(&mut text, "RERECORDS", &movie.rerecord_count.to_string());
        push_line(&mut text, "PLAYERS", "2");
        if let Some(author) = &movie.author
            && !author.is_empty()
        {
            push_line(&mut text, "AUTHOR", &escape(author));
        }
        if let Some(description) = &movie.description
            && !description.is_empty()
        {
            push_line(&mut text, "DESCRIPTION", &escape(description));
        }
        if let Some(state) = &movie.starting_state {
            text.push_str("\n[STATE]\n");
            text.push_str(&BASE64.encode(state));
            text.push('\n');
        }
        if !movie.markers.is_empty() {
            text.push_str("\n[MARKERS]\n");
            for marker in &movie.markers {
                text.push_str(&format!("{}|{}\n", marker.frame, escape(&marker.label)));
            }
        }
        if !movie.state_checksums.is_empty() {
            text.push_str("\n[CHECKSUMS]\n");
            for (frame, checksum) in &movie.state_checksums {
                text.push_str(&format!("{frame}|{checksum}\n"));
            }
        }
        text.push_str("\n[INPUT]\n");
        for (frame, input) in movie.frames.iter().enumerate() {
            text.push_str(&format!(
                "{frame}|{:02X}|{:02X}\n",
                input.player1, input.player2
            ));
        }
        Ok(text)
    }

    pub fn save(movie: &TasMovie, path: &Path) -> Result<(), TasFormatError> {
        let text = Self::serialize(movie)?;
        crate::persistence::atomic_write(path, text.as_bytes())?;
        Ok(())
    }
}

impl TasDeserializer {
    pub fn deserialize(text: &str, expected_rom: &str) -> Result<LoadedTas, TasFormatError> {
        #[derive(Clone, Copy, Eq, PartialEq)]
        enum Section {
            Metadata,
            State,
            Markers,
            Checksums,
            Input,
            Unknown,
        }

        let mut section = Section::Metadata;
        let mut metadata = BTreeMap::<String, String>::new();
        let mut state_text = String::new();
        let mut markers = Vec::new();
        let mut checksums = BTreeMap::new();
        let mut frames = Vec::new();
        let mut saw_input = false;
        for (line_number, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                section = match &line[1..line.len() - 1] {
                    "STATE" => Section::State,
                    "MARKERS" => Section::Markers,
                    "CHECKSUMS" => Section::Checksums,
                    "INPUT" => {
                        saw_input = true;
                        Section::Input
                    }
                    _ => Section::Unknown,
                };
                continue;
            }
            match section {
                Section::Metadata => {
                    let (key, value) = line.split_once(' ').ok_or_else(|| {
                        invalid_line(line_number, "metadata must contain a key and value")
                    })?;
                    metadata.insert(key.to_owned(), value.trim().to_owned());
                }
                Section::State => state_text.push_str(line),
                Section::Markers => {
                    let (frame, label) = line
                        .split_once('|')
                        .ok_or_else(|| invalid_line(line_number, "marker must be frame|label"))?;
                    markers.push(TasMarker {
                        frame: parse_usize(frame, line_number, "marker frame")?,
                        label: unescape(label)?,
                    });
                }
                Section::Checksums => {
                    let (frame, checksum) = line.split_once('|').ok_or_else(|| {
                        invalid_line(line_number, "checksum must be frame|sha256")
                    })?;
                    if !valid_sha256(checksum) {
                        return Err(invalid_line(line_number, "invalid state SHA-256"));
                    }
                    checksums.insert(
                        parse_usize(frame, line_number, "checksum frame")?,
                        checksum.to_ascii_lowercase(),
                    );
                }
                Section::Input => {
                    let fields: Vec<_> = line.split('|').collect();
                    if fields.len() != 3 {
                        return Err(invalid_line(
                            line_number,
                            "input must be frame|player1|player2",
                        ));
                    }
                    let frame = parse_usize(fields[0], line_number, "input frame")?;
                    if frame != frames.len() {
                        return Err(invalid_line(
                            line_number,
                            &format!("expected frame {}, got {frame}", frames.len()),
                        ));
                    }
                    frames.push(TasFrame {
                        player1: parse_mask(fields[1], line_number)?,
                        player2: parse_mask(fields[2], line_number)?,
                    });
                }
                Section::Unknown => {}
            }
        }

        let version = required(&metadata, "TAS_FORMAT")?
            .parse::<u32>()
            .map_err(|_| TasFormatError::Invalid("invalid TAS_FORMAT".into()))?;
        if version != FORMAT_VERSION {
            return Err(TasFormatError::UnsupportedVersion(version));
        }
        let emulator_name = required(&metadata, "EMULATOR")?;
        if emulator_name != EMULATOR_NAME && emulator_name != LEGACY_EMULATOR_NAME {
            return Err(TasFormatError::Invalid(
                "movie was created by an unknown emulator".into(),
            ));
        }
        if !saw_input {
            return Err(TasFormatError::Invalid("missing [INPUT] section".into()));
        }
        let actual_rom = required(&metadata, "ROM_SHA256")?.to_ascii_lowercase();
        if !valid_sha256(&actual_rom) {
            return Err(TasFormatError::Invalid("invalid ROM_SHA256".into()));
        }
        if actual_rom != expected_rom.to_ascii_lowercase() {
            return Err(TasFormatError::WrongRom {
                expected: expected_rom.to_owned(),
                actual: actual_rom,
            });
        }
        let emulator_version = required(&metadata, "EMULATOR_VERSION")?.to_owned();
        let region = match required(&metadata, "REGION")? {
            "NTSC" => Region::Ntsc,
            other => {
                return Err(TasFormatError::Invalid(format!(
                    "unsupported region {other}"
                )));
            }
        };
        let start_type = match required(&metadata, "START_TYPE")? {
            "POWER_ON" => TasStartType::PowerOn,
            "RESET" => TasStartType::Reset,
            "SAVE_STATE" => TasStartType::SaveState,
            other => {
                return Err(TasFormatError::Invalid(format!(
                    "unknown start type {other}"
                )));
            }
        };
        if required(&metadata, "PLAYERS")? != "2" {
            return Err(TasFormatError::Invalid(
                "version 1 requires PLAYERS 2".into(),
            ));
        }
        let starting_state = if state_text.is_empty() {
            None
        } else {
            Some(
                BASE64
                    .decode(state_text)
                    .map_err(|_| TasFormatError::Invalid("invalid base64 state".into()))?,
            )
        };
        if start_type == TasStartType::SaveState && starting_state.is_none() {
            return Err(TasFormatError::Invalid(
                "SAVE_STATE movie is missing [STATE] data".into(),
            ));
        }
        let rerecord_count = required(&metadata, "RERECORDS")?
            .parse::<u64>()
            .map_err(|_| TasFormatError::Invalid("invalid RERECORDS".into()))?;
        let mut warnings = Vec::new();
        if emulator_name == LEGACY_EMULATOR_NAME {
            warnings.push("movie uses the legacy pre-CrabNes emulator name".into());
        }
        if emulator_version != EMULATOR_VERSION {
            warnings.push(format!(
                "movie was created with emulator version {emulator_version}; current version is {EMULATOR_VERSION}"
            ));
        }
        let movie = TasMovie {
            format_version: version,
            rom_sha256: actual_rom,
            emulator_version,
            region,
            start_type,
            starting_state,
            rerecord_count,
            author: metadata
                .get("AUTHOR")
                .map(|value| unescape(value))
                .transpose()?,
            description: metadata
                .get("DESCRIPTION")
                .map(|value| unescape(value))
                .transpose()?,
            frames,
            markers,
            state_checksums: checksums,
        };
        validate_movie(&movie)?;
        Ok(LoadedTas { movie, warnings })
    }

    pub fn load(path: &Path, expected_rom: &str) -> Result<LoadedTas, TasFormatError> {
        let text = fs::read_to_string(path)?;
        Self::deserialize(&text, expected_rom)
    }
}

pub fn save(movie: &TasMovie, path: &Path) -> Result<(), TasFormatError> {
    TasSerializer::save(movie, path)
}

pub fn load(path: &Path, expected_rom: &str) -> Result<LoadedTas, TasFormatError> {
    TasDeserializer::load(path, expected_rom)
}

pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn rom_sha256_hex(hash: [u8; 32]) -> String {
    hash.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn validate_movie(movie: &TasMovie) -> Result<(), TasFormatError> {
    if movie.format_version != FORMAT_VERSION {
        return Err(TasFormatError::UnsupportedVersion(movie.format_version));
    }
    if !valid_sha256(&movie.rom_sha256) {
        return Err(TasFormatError::Invalid(
            "ROM SHA-256 must be 64 hex characters".into(),
        ));
    }
    if movie.start_type == TasStartType::SaveState && movie.starting_state.is_none() {
        return Err(TasFormatError::Invalid(
            "save-state-started movie has no embedded state".into(),
        ));
    }
    if movie
        .markers
        .iter()
        .any(|marker| marker.frame > movie.frames.len())
    {
        return Err(TasFormatError::Invalid(
            "marker points beyond the end of the movie".into(),
        ));
    }
    if movie
        .state_checksums
        .keys()
        .any(|frame| *frame > movie.frames.len())
    {
        return Err(TasFormatError::Invalid(
            "state checksum points beyond the end of the movie".into(),
        ));
    }
    Ok(())
}

fn push_line(text: &mut String, key: &str, value: &str) {
    text.push_str(key);
    text.push(' ');
    text.push_str(value);
    text.push('\n');
}

fn required<'a>(
    metadata: &'a BTreeMap<String, String>,
    key: &str,
) -> Result<&'a str, TasFormatError> {
    metadata
        .get(key)
        .map(String::as_str)
        .ok_or_else(|| TasFormatError::Invalid(format!("missing {key}")))
}

fn parse_usize(value: &str, line: usize, field: &str) -> Result<usize, TasFormatError> {
    value
        .parse()
        .map_err(|_| invalid_line(line, &format!("invalid {field}")))
}

fn parse_mask(value: &str, line: usize) -> Result<u8, TasFormatError> {
    if value.len() != 2 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid_line(line, "controller mask must be two hex digits"));
    }
    u8::from_str_radix(value, 16).map_err(|_| invalid_line(line, "invalid controller mask"))
}

fn invalid_line(line: usize, message: &str) -> TasFormatError {
    TasFormatError::Invalid(format!("line {}: {message}", line + 1))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn ordered(first: usize, second: usize) -> (usize, usize) {
    (first.min(second), first.max(second))
}

fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\r', "\\r")
        .replace('\n', "\\n")
}

fn unescape(value: &str) -> Result<String, TasFormatError> {
    let mut output = String::new();
    let mut chars = value.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            output.push(character);
            continue;
        }
        match chars.next() {
            Some('n') => output.push('\n'),
            Some('r') => output.push('\r'),
            Some('\\') => output.push('\\'),
            Some(other) => {
                return Err(TasFormatError::Invalid(format!(
                    "unknown escape sequence \\{other}"
                )));
            }
            None => return Err(TasFormatError::Invalid("trailing metadata escape".into())),
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn movie() -> TasMovie {
        let mut movie = TasMovie::new(
            "12".repeat(32),
            TasStartType::SaveState,
            Some(vec![1, 2, 3, 4]),
        );
        movie.author = Some("Jack".into());
        movie.description = Some("line one\nline two".into());
        movie.frames = vec![
            TasFrame {
                player1: 0x01,
                player2: 0x80,
            },
            TasFrame {
                player1: 0x01,
                player2: 0,
            },
            TasFrame::default(),
        ];
        movie.markers.push(TasMarker {
            frame: 1,
            label: "jump".into(),
        });
        movie.state_checksums.insert(0, "34".repeat(32));
        movie
    }

    #[test]
    fn readable_movie_round_trip_and_rom_validation() {
        let movie = movie();
        let text = TasSerializer::serialize(&movie).unwrap();
        assert!(text.contains("TAS_FORMAT 1"));
        assert!(text.contains("0|01|80"));
        let loaded = TasDeserializer::deserialize(&text, &movie.rom_sha256)
            .unwrap()
            .movie;
        assert_eq!(loaded.frames, movie.frames);
        assert_eq!(loaded.starting_state, movie.starting_state);
        assert_eq!(loaded.author, movie.author);
        assert_eq!(loaded.description, movie.description);
        assert!(matches!(
            TasDeserializer::deserialize(&text, &"99".repeat(32)),
            Err(TasFormatError::WrongRom { .. })
        ));
    }

    #[test]
    fn movie_file_round_trip() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir()
            .join(format!("crabnes-tas-{}-{nonce}", std::process::id()))
            .join("movie.tas");
        let movie = movie();
        save(&movie, &path).unwrap();
        assert_eq!(
            load(&path, &movie.rom_sha256).unwrap().movie.frames,
            movie.frames
        );
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn legacy_emulator_name_remains_compatible() {
        let movie = movie();
        let text = TasSerializer::serialize(&movie)
            .unwrap()
            .replace("EMULATOR CrabNes", "EMULATOR MyOwnNesEmulator");
        let loaded = TasDeserializer::deserialize(&text, &movie.rom_sha256).unwrap();
        assert_eq!(loaded.movie.frames, movie.frames);
        assert!(
            loaded
                .warnings
                .iter()
                .any(|warning| warning.contains("legacy"))
        );
    }

    #[test]
    fn rerecording_replaces_input_and_discards_the_old_future() {
        let mut manager = TasManager::default();
        let movie = TasMovie::new("12".repeat(32), TasStartType::PowerOn, None);
        manager.new_movie(movie, vec![0]);
        manager.input_for_frame(TasFrame {
            player1: 1,
            player2: 2,
        });
        manager.input_for_frame(TasFrame {
            player1: 3,
            player2: 4,
        });
        manager.input_for_frame(TasFrame {
            player1: 5,
            player2: 6,
        });
        manager.stop();
        manager.cursor = 1;
        assert!(manager.resume_recording());
        manager.input_for_frame(TasFrame {
            player1: 0x80,
            player2: 0x40,
        });
        let movie = manager.movie.unwrap();
        assert_eq!(movie.frames.len(), 2);
        assert_eq!(movie.frames[1].player1, 0x80);
        assert_eq!(movie.rerecord_count, 1);
    }

    #[test]
    fn playback_keeps_both_controllers_held() {
        let mut manager = TasManager::default();
        let mut movie = TasMovie::new("12".repeat(32), TasStartType::PowerOn, None);
        movie.frames = vec![
            TasFrame {
                player1: 1,
                player2: 0x80,
            },
            TasFrame {
                player1: 1,
                player2: 0x80,
            },
            TasFrame::default(),
        ];
        manager.install_movie(movie);
        manager.start_playback(false);
        assert_eq!(
            manager
                .input_for_frame(TasFrame::default())
                .unwrap()
                .player1,
            1
        );
        assert_eq!(
            manager
                .input_for_frame(TasFrame::default())
                .unwrap()
                .player2,
            0x80
        );
        assert_eq!(
            manager.input_for_frame(TasFrame::default()),
            Some(TasFrame::default())
        );
        assert_eq!(manager.input_for_frame(TasFrame::default()), None);
    }

    #[test]
    fn held_gui_input_is_combined_with_live_controller_input() {
        let live = TasFrame {
            player1: 0x01,
            player2: 0x10,
        };
        let held = TasFrame {
            player1: 0x88,
            player2: 0x02,
        };
        assert_eq!(
            live.with_held_input(held),
            TasFrame {
                player1: 0x89,
                player2: 0x12,
            }
        );
    }

    #[test]
    fn seeking_a_recording_previews_movie_input_instead_of_overwriting_it() {
        let mut manager = TasManager::default();
        let mut movie = TasMovie::new("12".repeat(32), TasStartType::PowerOn, None);
        movie.frames.push(TasFrame {
            player1: 0x01,
            player2: 0x80,
        });
        manager.new_movie(movie, vec![0]);
        manager.set_cursor_paused_for_preview(0);

        let preview = manager
            .input_for_frame(TasFrame {
                player1: 0x40,
                player2: 0x20,
            })
            .unwrap();
        assert_eq!(preview.player1, 0x01);
        assert_eq!(preview.player2, 0x80);
        assert_eq!(manager.movie.as_ref().unwrap().frames[0], preview);
        assert_eq!(manager.cursor, 1);

        manager.set_cursor_paused_for_preview(1);
        assert_eq!(manager.selected_frame, 1);
        assert_eq!(manager.range_end_frame, 1);
    }

    #[test]
    fn frame_advance_previews_existing_rows_then_records_new_end_rows() {
        let mut manager = TasManager::default();
        let mut movie = TasMovie::new("12".repeat(32), TasStartType::PowerOn, None);
        movie.frames.push(TasFrame {
            player1: 0x08,
            player2: 0,
        });
        manager.install_movie(movie);

        assert!(manager.prepare_frame_advance());
        assert_eq!(
            manager
                .input_for_frame(TasFrame::default())
                .unwrap()
                .player1,
            0x08
        );
        assert_eq!(manager.cursor, 1);

        assert!(manager.prepare_frame_advance());
        manager.pause();
        assert!(manager.recording_context());
        assert_eq!(
            manager.input_for_frame(TasFrame::default()),
            Some(TasFrame::default())
        );
        assert_eq!(manager.cursor, 2);
        assert_eq!(manager.movie.as_ref().unwrap().frames.len(), 2);
    }

    #[test]
    fn recording_rewind_removes_future_input_and_checkpoints() {
        let mut manager = TasManager::default();
        let mut movie = TasMovie::new("12".repeat(32), TasStartType::PowerOn, None);
        movie.markers.push(TasMarker {
            frame: 1,
            label: "keep".into(),
        });
        movie.markers.push(TasMarker {
            frame: 3,
            label: "remove".into(),
        });
        manager.new_movie(movie, vec![0]);
        for input in 1..=4 {
            manager.input_for_frame(TasFrame {
                player1: input,
                player2: 0,
            });
        }
        manager.checkpoints.push(TasCheckpoint {
            frame: 2,
            state: vec![2],
        });
        manager.checkpoints.push(TasCheckpoint {
            frame: 4,
            state: vec![4],
        });
        manager.pause();

        assert_eq!(manager.truncate_recording_at(2), 2);
        let movie = manager.movie.as_ref().unwrap();
        assert_eq!(movie.frames.len(), 2);
        assert_eq!(movie.rerecord_count, 1);
        assert_eq!(movie.markers.len(), 1);
        assert!(manager.checkpoints.iter().all(|point| point.frame <= 2));
        assert_eq!(manager.cursor, 2);
        assert_eq!(manager.mode, TasMode::Paused);
        assert!(manager.recording_context());
    }

    #[test]
    fn parser_rejects_bad_frame_numbers_and_masks() {
        let text = TasSerializer::serialize(&movie()).unwrap();
        let bad_frame = text.replace("1|01|00", "9|01|00");
        assert!(TasDeserializer::deserialize(&bad_frame, &"12".repeat(32)).is_err());
        let bad_mask = text.replace("0|01|80", "0|GG|80");
        assert!(TasDeserializer::deserialize(&bad_mask, &"12".repeat(32)).is_err());
    }

    #[test]
    fn unknown_optional_metadata_is_ignored_and_version_difference_warns() {
        let movie = movie();
        let mut text = TasSerializer::serialize(&movie).unwrap().replace(
            &format!("EMULATOR_VERSION {EMULATOR_VERSION}"),
            "EMULATOR_VERSION 0.0.9\nFUTURE_OPTION enabled",
        );
        text.push_str("\n[UNKNOWN]\nanything\n");
        let loaded = TasDeserializer::deserialize(&text, &movie.rom_sha256).unwrap();
        assert_eq!(loaded.movie.frames, movie.frames);
        assert_eq!(loaded.warnings.len(), 1);
    }

    #[test]
    fn editor_supports_insert_delete_copy_and_paste() {
        let mut manager = TasManager::default();
        let mut movie = TasMovie::new("12".repeat(32), TasStartType::PowerOn, None);
        movie.frames = vec![
            TasFrame {
                player1: 1,
                player2: 2,
            },
            TasFrame {
                player1: 3,
                player2: 4,
            },
        ];
        movie.markers.push(TasMarker {
            frame: 1,
            label: "second".into(),
        });
        manager.install_movie(movie);
        manager.selected_frame = 0;
        manager.range_end_frame = 1;
        assert!(manager.copy_selection());
        manager.selected_frame = 0;
        assert!(manager.paste_selection(true));
        assert_eq!(manager.movie.as_ref().unwrap().frames.len(), 4);
        assert_eq!(manager.movie.as_ref().unwrap().markers[0].frame, 3);
        assert!(TasEditor::delete(manager.movie.as_mut().unwrap(), 1, 2));
        assert_eq!(manager.movie.as_ref().unwrap().frames.len(), 2);
        assert_eq!(manager.movie.as_ref().unwrap().markers[0].frame, 1);
    }

    #[test]
    fn checkpoint_hash_detects_a_desync() {
        let state = vec![1, 2, 3];
        let mut movie = TasMovie::new("12".repeat(32), TasStartType::PowerOn, None);
        movie.state_checksums.insert(0, sha256_hex(&state));
        movie.state_checksums.insert(300, sha256_hex(&state));
        let mut manager = TasManager::default();
        manager.install_movie(movie);
        manager.start_playback(false);
        assert!(manager.maybe_checkpoint(0, vec![9, 9, 9]));
        assert!(
            manager
                .last_desync
                .as_deref()
                .is_some_and(|message| message.contains("frame 0"))
        );

        // A latched earlier error must not hide the next mismatch from the
        // playback recovery path.
        assert!(manager.maybe_checkpoint(300, vec![8, 8, 8]));
        assert!(
            manager
                .last_desync
                .as_deref()
                .is_some_and(|message| message.contains("frame 300"))
        );

        let verified = vec![4, 5, 6];
        manager.repair_checkpoint_checksum(0, &verified);
        assert!(manager.last_desync.is_none());
        assert_eq!(
            manager.movie.as_ref().unwrap().state_checksums[&0],
            sha256_hex(&verified)
        );
    }

    #[test]
    fn checkpoint_reconciliation_refreshes_resyncs_or_stops_safely() {
        let expected_state = vec![1, 2, 3];
        let divergent_state = vec![9, 9, 9];
        let expected = sha256_hex(&expected_state);

        assert_eq!(
            reconcile_checkpoint(&expected, &divergent_state, &divergent_state),
            CheckpointReconciliation::RefreshChecksum
        );
        assert_eq!(
            reconcile_checkpoint(&expected, &divergent_state, &expected_state),
            CheckpointReconciliation::RestoreReplay
        );
        assert_eq!(
            reconcile_checkpoint(&expected, &divergent_state, &[4, 5, 6]),
            CheckpointReconciliation::Unrecoverable
        );
    }
}

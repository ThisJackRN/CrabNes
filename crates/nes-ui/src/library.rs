use std::{
    collections::{HashSet, VecDeque},
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::persistence;
use nes_core::{Nes, Region};

const LIBRARY_VERSION: u32 = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoverSource {
    Custom,
    RetroAchievements,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccuracyEstimate {
    pub score: u8,
    pub rating: String,
    pub mapper_coverage: String,
    pub passed: u16,
    pub total: u16,
    pub details: Vec<String>,
}

const ACCURACYCOIN_PASSED: u16 = 101;
const ACCURACYCOIN_TOTAL: u16 = 141;
const ACCURACYCOIN_OFFICIAL_CPU: (u16, u16, u8) = (22, 23, 10);
const ACCURACYCOIN_UNOFFICIAL_CPU: (u16, u16, u8) = (66, 66, 5);
const ACCURACYCOIN_PPU: (u16, u16, u8) = (8, 33, 15);
const ACCURACYCOIN_APU_DMA: (u16, u16, u8) = (5, 19, 5);

#[derive(Clone)]
pub struct LibraryEntry {
    pub title: String,
    pub file_name: String,
    pub path: PathBuf,
    pub last_played: u64,
    pub cover_image: Option<PathBuf>,
    pub cover_source: Option<CoverSource>,
    pub accuracy: Option<AccuracyEstimate>,
    pub status: EntryStatus,
}

#[derive(Clone)]
pub enum EntryStatus {
    Ready,
    Missing,
    Invalid(String),
}

#[derive(Default, Serialize, Deserialize)]
struct LibraryFile {
    version: u32,
    opened: Vec<StoredEntry>,
    #[serde(default)]
    hidden: Vec<PathBuf>,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredEntry {
    path: PathBuf,
    last_played: u64,
    #[serde(default)]
    custom_title: Option<String>,
    #[serde(default)]
    cover_image: Option<PathBuf>,
    #[serde(default)]
    retro_cover_image: Option<PathBuf>,
}

pub struct RomLibrary {
    stored: Vec<StoredEntry>,
    hidden: Vec<PathBuf>,
    pub entries: Vec<LibraryEntry>,
}

impl RomLibrary {
    pub fn load() -> Self {
        let file = fs::read(library_path())
            .ok()
            .and_then(|data| serde_json::from_slice::<LibraryFile>(&data).ok())
            .unwrap_or_default();
        let mut library = Self {
            stored: file.opened,
            hidden: file.hidden,
            entries: Vec::new(),
        };
        library.refresh(None);
        library
    }

    pub fn refresh(&mut self, folder: Option<&Path>) {
        struct Candidate {
            path: PathBuf,
            last_played: u64,
            custom_title: Option<String>,
            custom_cover: Option<PathBuf>,
            retro_cover: Option<PathBuf>,
        }

        let mut paths: Vec<Candidate> = self
            .stored
            .iter()
            .map(|entry| Candidate {
                path: entry.path.clone(),
                last_played: entry.last_played,
                custom_title: entry.custom_title.clone(),
                custom_cover: entry.cover_image.clone(),
                retro_cover: entry.retro_cover_image.clone(),
            })
            .collect();
        if let Some(folder) = folder
            && let Ok(read_dir) = fs::read_dir(folder)
        {
            for item in read_dir.flatten() {
                let path = item.path();
                if path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("nes"))
                    && !self
                        .hidden
                        .iter()
                        .any(|hidden| normalize(hidden) == normalize(&path))
                {
                    paths.push(Candidate {
                        path,
                        last_played: 0,
                        custom_title: None,
                        custom_cover: None,
                        retro_cover: None,
                    });
                }
            }
        }
        let mut seen = HashSet::new();
        let mut seen_roms = HashSet::new();
        self.entries.clear();
        for candidate in paths {
            let Candidate {
                path,
                last_played,
                custom_title,
                custom_cover,
                retro_cover,
            } = candidate;
            let key = normalize(&path);
            if !seen.insert(key) {
                continue;
            }
            let file_name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("Unknown")
                .to_owned();
            let default_title = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Unknown")
                .replace(['_', '-'], " ");
            let title = custom_title
                .filter(|title| !title.trim().is_empty())
                .unwrap_or(default_title);
            let (status, rom_hash, accuracy) = if !path.is_file() {
                (EntryStatus::Missing, None, None)
            } else {
                match fs::read(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|bytes| {
                        Nes::from_ines(&bytes)
                            .map(|nes| {
                                let accuracy = estimate_accuracy(&nes, &bytes);
                                (nes.rom_hash(), accuracy)
                            })
                            .map_err(|e| e.to_string())
                    }) {
                    Ok((hash, accuracy)) => (EntryStatus::Ready, Some(hash), Some(accuracy)),
                    Err(error) => (EntryStatus::Invalid(error), None, None),
                }
            };
            if rom_hash.is_some_and(|hash| !seen_roms.insert(hash)) {
                continue;
            }
            let (cover_image, cover_source) = effective_cover(custom_cover, retro_cover);
            self.entries.push(LibraryEntry {
                title,
                file_name,
                path,
                last_played,
                cover_image,
                cover_source,
                accuracy,
                status,
            });
        }
        let available_names: HashSet<String> = self
            .entries
            .iter()
            .filter(|entry| matches!(&entry.status, EntryStatus::Ready))
            .map(|entry| entry.file_name.to_lowercase())
            .collect();
        self.entries.retain(|entry| {
            !matches!(&entry.status, EntryStatus::Missing)
                || !available_names.contains(&entry.file_name.to_lowercase())
        });
    }

    pub fn remember(&mut self, path: &Path) -> io::Result<()> {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let key = normalize(&path);
        self.hidden.retain(|hidden| normalize(hidden) != key);
        let existing = self
            .stored
            .iter()
            .find(|entry| normalize(&entry.path) == key)
            .cloned();
        self.stored.retain(|entry| normalize(&entry.path) != key);
        self.stored.insert(
            0,
            StoredEntry {
                path,
                last_played: now,
                custom_title: existing
                    .as_ref()
                    .and_then(|entry| entry.custom_title.clone()),
                cover_image: existing
                    .as_ref()
                    .and_then(|entry| entry.cover_image.clone()),
                retro_cover_image: existing.and_then(|entry| entry.retro_cover_image),
            },
        );
        self.save()
    }

    pub fn recent(&self) -> Vec<&LibraryEntry> {
        let mut entries: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| entry.last_played > 0)
            .collect();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.last_played));
        entries
    }

    pub fn forget(&mut self, path: &Path) -> io::Result<()> {
        let key = normalize(path);
        let covers = self
            .stored
            .iter()
            .find(|entry| normalize(&entry.path) == key)
            .map(|entry| [entry.cover_image.clone(), entry.retro_cover_image.clone()])
            .unwrap_or_default();
        self.stored.retain(|entry| normalize(&entry.path) != key);
        self.entries.retain(|entry| normalize(&entry.path) != key);
        if !self.hidden.iter().any(|hidden| normalize(hidden) == key) {
            self.hidden.push(path.to_path_buf());
        }
        self.save()?;
        for cover in covers.into_iter().flatten() {
            remove_managed_cover(&cover);
        }
        Ok(())
    }

    pub fn rename(&mut self, path: &Path, title: &str) -> io::Result<()> {
        let title = title.trim();
        let custom_title = (!title.is_empty()).then(|| title.to_owned());
        let stored = self.ensure_stored(path);
        stored.custom_title = custom_title.clone();
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| normalize(&entry.path) == normalize(path))
        {
            entry.title = custom_title.unwrap_or_else(|| default_title(path));
        }
        self.save()
    }

    pub fn set_cover_image(&mut self, path: &Path, source: &Path) -> io::Result<PathBuf> {
        fs::create_dir_all(cover_directory())?;
        let extension = source
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("png")
            .to_ascii_lowercase();
        let digest = Sha256::digest(normalize(path).as_bytes());
        let id = digest[..12]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let destination = cover_directory().join(format!("{id}.{extension}"));
        fs::copy(source, &destination)?;
        let old_cover = self
            .ensure_stored(path)
            .cover_image
            .replace(destination.clone());
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| normalize(&entry.path) == normalize(path))
        {
            entry.cover_image = Some(destination.clone());
            entry.cover_source = Some(CoverSource::Custom);
        }
        self.save()?;
        if let Some(old_cover) = old_cover
            && old_cover != destination
            && old_cover.starts_with(cover_directory())
        {
            let _ = fs::remove_file(old_cover);
        }
        Ok(destination)
    }

    pub fn remove_cover_image(&mut self, path: &Path) -> io::Result<()> {
        let (old_cover, retro_cover) = {
            let stored = self.ensure_stored(path);
            (
                stored.cover_image.take(),
                stored
                    .retro_cover_image
                    .clone()
                    .filter(|image| image.is_file()),
            )
        };
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| normalize(&entry.path) == normalize(path))
        {
            entry.cover_image = retro_cover;
            entry.cover_source = entry
                .cover_image
                .as_ref()
                .map(|_| CoverSource::RetroAchievements);
        }
        self.save()?;
        if let Some(old_cover) = old_cover
            && old_cover.starts_with(cover_directory())
        {
            let _ = fs::remove_file(old_cover);
        }
        Ok(())
    }

    pub fn has_retro_cover(&self, path: &Path) -> bool {
        let key = normalize(path);
        self.stored
            .iter()
            .find(|entry| normalize(&entry.path) == key)
            .and_then(|entry| entry.retro_cover_image.as_ref())
            .is_some_and(|cover| cover.is_file())
    }

    pub fn set_retro_cover_image(
        &mut self,
        path: &Path,
        size: [usize; 2],
        rgba: &[u8],
    ) -> io::Result<PathBuf> {
        let expected_len = size[0]
            .checked_mul(size[1])
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "image is too large"))?;
        if size.contains(&0) || rgba.len() != expected_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid RGBA image dimensions",
            ));
        }
        let width = u32::try_from(size[0])
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "image is too wide"))?;
        let height = u32::try_from(size[1])
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "image is too tall"))?;
        fs::create_dir_all(cover_directory())?;
        let destination =
            cover_directory().join(format!("{}-retroachievements.png", cover_id(path)));
        image::save_buffer_with_format(
            &destination,
            rgba,
            width,
            height,
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .map_err(io::Error::other)?;

        let (old_cover, custom_active) = {
            let stored = self.ensure_stored(path);
            let old_cover = stored.retro_cover_image.replace(destination.clone());
            let custom_active = stored
                .cover_image
                .as_ref()
                .is_some_and(|cover| cover.is_file());
            (old_cover, custom_active)
        };
        if !custom_active
            && let Some(entry) = self
                .entries
                .iter_mut()
                .find(|entry| normalize(&entry.path) == normalize(path))
        {
            entry.cover_image = Some(destination.clone());
            entry.cover_source = Some(CoverSource::RetroAchievements);
        }
        self.save()?;
        if let Some(old_cover) = old_cover
            && old_cover != destination
        {
            remove_managed_cover(&old_cover);
        }
        Ok(destination)
    }

    fn save(&self) -> io::Result<()> {
        let file = LibraryFile {
            version: LIBRARY_VERSION,
            opened: self.stored.clone(),
            hidden: self.hidden.clone(),
        };
        let data = serde_json::to_vec_pretty(&file).map_err(io::Error::other)?;
        persistence::atomic_write(&library_path(), &data)
    }

    fn ensure_stored(&mut self, path: &Path) -> &mut StoredEntry {
        let key = normalize(path);
        if let Some(index) = self
            .stored
            .iter()
            .position(|entry| normalize(&entry.path) == key)
        {
            return &mut self.stored[index];
        }
        let last_played = self
            .entries
            .iter()
            .find(|entry| normalize(&entry.path) == key)
            .map_or(0, |entry| entry.last_played);
        self.stored.push(StoredEntry {
            path: path.to_path_buf(),
            last_played,
            custom_title: None,
            cover_image: None,
            retro_cover_image: None,
        });
        self.stored.last_mut().unwrap()
    }
}

fn estimate_accuracy(nes: &Nes, bytes: &[u8]) -> AccuracyEstimate {
    let mapper = nes.mapper_id();
    let (coverage, mapper_penalty, mapper_note): (&str, u8, &str) = match mapper {
        0 => (
            "Focused",
            0,
            "NROM has no bank-switching hardware and receives the strongest coverage.",
        ),
        1 => (
            "Focused",
            2,
            "MMC1 banking, mirroring, RAM, and serial writes are covered.",
        ),
        2 | 3 | 7 => (
            "Focused",
            2,
            "This discrete mapper family has focused banking and mirroring tests.",
        ),
        9 | 10 => (
            "Partial",
            5,
            "MMC2/MMC4 banking and latch behavior have focused regressions, but the PPU fetch pipeline is not yet dot-perfect.",
        ),
        4 => (
            "Broad",
            5,
            "MMC3 is broadly supported, but exact IRQ and MMC6 board variants remain accuracy work.",
        ),
        21 | 22 | 23 | 25 => (
            "Partial",
            7,
            "VRC2/VRC4 banking and cycle IRQs work; wiring variants reduce confidence.",
        ),
        24 | 26 => (
            "Partial",
            7,
            "VRC6 banking, IRQs, and expansion audio work; analog audio matching is approximate.",
        ),
        69 => (
            "Partial",
            7,
            "FME-7 banking, IRQs, and Sunsoft 5B audio work; uncommon board variants need testing.",
        ),
        19 => (
            "Partial",
            8,
            "Namco 163 banking, IRQs, and audio work; channel mixing remains approximate.",
        ),
        5 => (
            "Experimental",
            12,
            "MMC5 works, but extended attributes and vertical split rendering remain incomplete.",
        ),
        85 => (
            "Experimental",
            14,
            "VRC7 works, but FM operator and envelope behavior is still experimental.",
        ),
        99 => (
            "Partial",
            10,
            "Nintendo Vs. System PRG/CHR banking, four-screen nametables, single-coin input, and the known Vs. Super Mario Bros. RGB palette are covered; configurable DIP switches and other RGB PPU revisions remain incomplete.",
        ),
        _ => (
            "Experimental",
            20,
            "This mapper does not have a dedicated CrabNes confidence profile yet.",
        ),
    };
    let requirements = analyze_rom_requirements(bytes, mapper);
    let official_cpu_penalty = accuracycoin_penalty(ACCURACYCOIN_OFFICIAL_CPU);
    let unofficial_cpu_penalty = accuracycoin_penalty(ACCURACYCOIN_UNOFFICIAL_CPU);
    let ppu_penalty = scale_penalty(
        accuracycoin_penalty(ACCURACYCOIN_PPU),
        requirements.ppu_exposure(),
    );
    let apu_dma_penalty = scale_penalty(
        accuracycoin_penalty(ACCURACYCOIN_APU_DMA),
        requirements.apu_dma_exposure(),
    );
    let common_score = 100_u8
        .saturating_sub(official_cpu_penalty)
        .saturating_sub(unofficial_cpu_penalty)
        .saturating_sub(ppu_penalty)
        .saturating_sub(apu_dma_penalty);
    let mut score = common_score.saturating_sub(mapper_penalty);
    let mut details = vec![
        format!(
            "AccuracyCoin result: {ACCURACYCOIN_PASSED}/{ACCURACYCOIN_TOTAL} tests pass overall."
        ),
        format!(
            "Likely-code hardware score: {common_score}/100 (CPU -{}, PPU -{} at {}% exposure, APU/DMA -{} at {}% exposure).",
            official_cpu_penalty + unofficial_cpu_penalty,
            ppu_penalty,
            requirements.ppu_exposure(),
            apu_dma_penalty,
            requirements.apu_dma_exposure(),
        ),
        requirements.analysis_note(mapper),
        format!("Mapper {mapper} coverage: {coverage}; -{mapper_penalty} points. {mapper_note}"),
    ];

    let nes2 = bytes.get(7).is_some_and(|value| value & 0x0c == 0x08);
    let submapper = if nes2 {
        bytes.get(8).map_or(0, |value| value >> 4)
    } else {
        0
    };
    if nes2 {
        details.push(format!("NES 2.0 header; submapper {submapper}."));
        if submapper != 0 && !matches!(mapper, 19 | 21 | 22 | 23 | 25 | 85) {
            score = score.saturating_sub(6);
            details.push("Unknown specialized NES 2.0 submapper: -6 points.".into());
        }
    } else {
        details.push("iNES header; hardware variants may not be fully identified.".into());
    }

    match nes.region() {
        Region::Ntsc => details.push("NTSC cycle timing is enabled.".into()),
        Region::Pal => {
            score = score.saturating_sub(8);
            details.push(
                "PAL timing: -8 points because AccuracyCoin measures NTSC RP2A03G/RP2C02G behavior."
                    .into(),
            );
        }
    }
    if bytes.get(6).is_some_and(|value| value & 0x08 != 0) {
        score = score.saturating_sub(4);
        details.push("Four-screen nametable hardware: -4 points due to limited coverage.".into());
    }
    details.push(
        "This is a static estimate derived from likely reachable code, AccuracyCoin categories, and cartridge requirements—not an exhaustive playthrough."
            .into(),
    );

    AccuracyEstimate {
        score,
        rating: match score {
            80..=u8::MAX => "High",
            70..=79 => "Good",
            60..=69 => "Fair",
            _ => "Experimental",
        }
        .into(),
        mapper_coverage: coverage.into(),
        passed: ACCURACYCOIN_PASSED,
        total: ACCURACYCOIN_TOTAL,
        details,
    }
}

fn accuracycoin_penalty((passed, total, risk_budget): (u16, u16, u8)) -> u8 {
    let failed = total.saturating_sub(passed);
    ((u32::from(failed) * u32::from(risk_budget) + u32::from(total) / 2) / u32::from(total)) as u8
}

fn scale_penalty(penalty: u8, exposure_percent: u8) -> u8 {
    ((u16::from(penalty) * u16::from(exposure_percent) + 50) / 100) as u8
}

#[derive(Default)]
struct RomRequirements {
    instructions: usize,
    ppu_control: bool,
    ppu_status: bool,
    ppu_oam: bool,
    ppu_scroll_vram: bool,
    apu_channels: bool,
    apu_enable: bool,
    apu_frame_counter: bool,
    dmc: bool,
}

impl RomRequirements {
    fn ppu_exposure(&self) -> u8 {
        if self.instructions == 0 {
            return 100;
        }
        u8::from(self.ppu_control) * 15
            + u8::from(self.ppu_status) * 15
            + u8::from(self.ppu_oam) * 15
            + u8::from(self.ppu_scroll_vram) * 20
    }

    fn apu_dma_exposure(&self) -> u8 {
        if self.instructions == 0 || self.dmc {
            return 100;
        }
        u8::from(self.apu_channels) * 30
            + u8::from(self.apu_enable) * 10
            + u8::from(self.apu_frame_counter) * 10
    }

    fn analysis_note(&self, mapper: u16) -> String {
        if self.instructions == 0 {
            return "Static code scan could not resolve the interrupt vectors; full shared hardware risk was retained.".into();
        }
        let scope = match mapper {
            0 => "the complete fixed NROM program",
            9 => "fixed code and MMC2 8 KiB banks reached from fixed-bank calls",
            _ => {
                "the startup and fixed banks; dynamically selected banks remain covered by mapper risk"
            }
        };
        format!(
            "Static reachability scan decoded {} likely instructions from the reset/NMI/IRQ vectors across {scope}.",
            self.instructions
        )
    }

    fn record_access(&mut self, address: u16, write: bool, indexed: bool) {
        if (0x2000..=0x3fff).contains(&address) {
            if indexed {
                self.ppu_control = true;
                self.ppu_status = true;
                self.ppu_oam = true;
                self.ppu_scroll_vram = true;
                return;
            }
            match address & 7 {
                0 | 1 => self.ppu_control = true,
                2 => self.ppu_status = true,
                3 | 4 => self.ppu_oam = true,
                5..=7 => self.ppu_scroll_vram = true,
                _ => {}
            }
        }
        match address {
            0x4000..=0x400f => self.apu_channels = true,
            0x4010..=0x4013 => self.dmc = true,
            0x4014 if write => self.ppu_oam = true,
            0x4015 => self.apu_enable = true,
            0x4017 if write => self.apu_frame_counter = true,
            _ => {}
        }
    }
}

fn analyze_rom_requirements(bytes: &[u8], mapper: u16) -> RomRequirements {
    let Some(prg) = ines_prg(bytes) else {
        return RomRequirements::default();
    };
    if prg.len() < 6 {
        return RomRequirements::default();
    }

    let mut requirements = RomRequirements::default();
    let mut queue = VecDeque::new();
    for vector_offset in [prg.len() - 6, prg.len() - 4, prg.len() - 2] {
        let vector = u16::from_le_bytes([prg[vector_offset], prg[vector_offset + 1]]);
        if vector >= 0x8000 {
            queue.push_back((vector, None));
        }
    }
    let mut visited = HashSet::new();

    while let Some((pc, selected_bank)) = queue.pop_front() {
        if !visited.insert((pc, selected_bank)) || visited.len() > 262_144 {
            continue;
        }
        let Some(opcode) = read_prg_byte(prg, mapper, pc, selected_bank) else {
            continue;
        };
        let length = usize::from(OPCODE_LENGTHS[opcode as usize]);
        let operand_low = (length >= 2)
            .then(|| read_prg_byte(prg, mapper, pc.wrapping_add(1), selected_bank))
            .flatten();
        let operand_high = (length >= 3)
            .then(|| read_prg_byte(prg, mapper, pc.wrapping_add(2), selected_bank))
            .flatten();
        if length >= 2 && operand_low.is_none() || length >= 3 && operand_high.is_none() {
            continue;
        }
        requirements.instructions += 1;
        let next = pc.wrapping_add(length as u16);

        if length == 3 && !matches!(opcode, 0x20 | 0x4c | 0x6c) {
            let address = u16::from_le_bytes([operand_low.unwrap(), operand_high.unwrap()]);
            requirements.record_access(
                address,
                opcode_writes_memory(opcode),
                opcode_is_indexed(opcode),
            );
        }

        match opcode {
            0x00 | 0x40 | 0x60 | 0x02 | 0x12 | 0x22 | 0x32 | 0x42 | 0x52 | 0x62 | 0x72 | 0x92
            | 0xb2 | 0xd2 | 0xf2 => {}
            0x4c => enqueue_code_target(
                &mut queue,
                prg,
                mapper,
                pc,
                u16::from_le_bytes([operand_low.unwrap(), operand_high.unwrap()]),
                selected_bank,
            ),
            0x6c => {}
            0x20 => {
                enqueue_code_target(
                    &mut queue,
                    prg,
                    mapper,
                    pc,
                    u16::from_le_bytes([operand_low.unwrap(), operand_high.unwrap()]),
                    selected_bank,
                );
                queue.push_back((next, selected_bank));
            }
            0x10 | 0x30 | 0x50 | 0x70 | 0x90 | 0xb0 | 0xd0 | 0xf0 => {
                queue.push_back((
                    next.wrapping_add_signed(i16::from(operand_low.unwrap() as i8)),
                    selected_bank,
                ));
                queue.push_back((next, selected_bank));
            }
            _ => queue.push_back((next, selected_bank)),
        }
    }
    requirements
}

fn ines_prg(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.get(0..4)? != b"NES\x1a" {
        return None;
    }
    let start = 16 + usize::from(bytes.get(6)? & 0x04 != 0) * 512;
    let declared = usize::from(*bytes.get(4)?) * 0x4000;
    bytes.get(start..start.checked_add(declared)?)
}

fn enqueue_code_target(
    queue: &mut VecDeque<(u16, Option<usize>)>,
    prg: &[u8],
    mapper: u16,
    source: u16,
    target: u16,
    selected_bank: Option<usize>,
) {
    if mapper == 9 && source >= 0xa000 && (0x8000..0xa000).contains(&target) {
        for bank in 0..prg.len() / 0x2000 {
            queue.push_back((target, Some(bank)));
        }
    } else {
        queue.push_back((target, selected_bank));
    }
}

fn read_prg_byte(
    prg: &[u8],
    mapper: u16,
    address: u16,
    selected_bank: Option<usize>,
) -> Option<u8> {
    if address < 0x8000 || prg.is_empty() {
        return None;
    }
    let cpu_offset = usize::from(address - 0x8000);
    let offset = if mapper == 9 {
        let bank_count = prg.len() / 0x2000;
        let slot = usize::from((address - 0x8000) / 0x2000);
        let bank = if slot == 0 {
            selected_bank.unwrap_or(0) % bank_count
        } else {
            bank_count.checked_sub(4)? + slot
        };
        bank * 0x2000 + usize::from(address & 0x1fff)
    } else if prg.len() <= 0x4000 {
        cpu_offset % prg.len()
    } else if mapper == 0 && prg.len() <= 0x8000 {
        cpu_offset % prg.len()
    } else if address >= 0xc000 {
        prg.len().checked_sub(0x4000)? + usize::from(address - 0xc000)
    } else {
        cpu_offset % 0x4000
    };
    prg.get(offset).copied()
}

fn opcode_writes_memory(opcode: u8) -> bool {
    matches!(
        opcode,
        0x0e | 0x0f
            | 0x1b
            | 0x1e
            | 0x1f
            | 0x2e
            | 0x2f
            | 0x3b
            | 0x3e
            | 0x3f
            | 0x4e
            | 0x4f
            | 0x5b
            | 0x5e
            | 0x5f
            | 0x6e
            | 0x6f
            | 0x7b
            | 0x7e
            | 0x7f
            | 0x8c
            | 0x8d
            | 0x8e
            | 0x8f
            | 0x99
            | 0x9b
            | 0x9c
            | 0x9d
            | 0x9e
            | 0x9f
            | 0xce
            | 0xcf
            | 0xdb
            | 0xde
            | 0xdf
            | 0xee
            | 0xef
            | 0xfb
            | 0xfe
            | 0xff
    )
}

fn opcode_is_indexed(opcode: u8) -> bool {
    matches!(
        opcode,
        0x19 | 0x1b
            | 0x1d
            | 0x1e
            | 0x1f
            | 0x39
            | 0x3b
            | 0x3d
            | 0x3e
            | 0x3f
            | 0x59
            | 0x5b
            | 0x5d
            | 0x5e
            | 0x5f
            | 0x79
            | 0x7b
            | 0x7d
            | 0x7e
            | 0x7f
            | 0x99
            | 0x9b
            | 0x9c
            | 0x9d
            | 0x9e
            | 0x9f
            | 0xb9
            | 0xbb
            | 0xbc
            | 0xbd
            | 0xbe
            | 0xbf
            | 0xd9
            | 0xdb
            | 0xdd
            | 0xde
            | 0xdf
            | 0xf9
            | 0xfb
            | 0xfd
            | 0xfe
            | 0xff
    )
}

#[rustfmt::skip]
const OPCODE_LENGTHS: [u8; 256] = [
    1,2,1,2,2,2,2,2,1,2,1,2,3,3,3,3, 2,2,1,2,2,2,2,2,1,3,1,3,3,3,3,3,
    3,2,1,2,2,2,2,2,1,2,1,2,3,3,3,3, 2,2,1,2,2,2,2,2,1,3,1,3,3,3,3,3,
    1,2,1,2,2,2,2,2,1,2,1,2,3,3,3,3, 2,2,1,2,2,2,2,2,1,3,1,3,3,3,3,3,
    1,2,1,2,2,2,2,2,1,2,1,2,3,3,3,3, 2,2,1,2,2,2,2,2,1,3,1,3,3,3,3,3,
    2,2,2,2,2,2,2,2,1,2,1,2,3,3,3,3, 2,2,1,2,2,2,2,2,1,3,1,3,3,3,3,3,
    2,2,2,2,2,2,2,2,1,2,1,2,3,3,3,3, 2,2,1,2,2,2,2,2,1,3,1,3,3,3,3,3,
    2,2,2,2,2,2,2,2,1,2,1,2,3,3,3,3, 2,2,1,2,2,2,2,2,1,3,1,3,3,3,3,3,
    2,2,2,2,2,2,2,2,1,2,1,2,3,3,3,3, 2,2,1,2,2,2,2,2,1,3,1,3,3,3,3,3,
];

fn default_title(path: &Path) -> String {
    path.file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("Unknown")
        .replace(['_', '-'], " ")
}

fn cover_id(path: &Path) -> String {
    let digest = Sha256::digest(normalize(path).as_bytes());
    digest[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn effective_cover(
    custom: Option<PathBuf>,
    retro: Option<PathBuf>,
) -> (Option<PathBuf>, Option<CoverSource>) {
    if let Some(custom) = custom.filter(|image| image.is_file()) {
        (Some(custom), Some(CoverSource::Custom))
    } else if let Some(retro) = retro.filter(|image| image.is_file()) {
        (Some(retro), Some(CoverSource::RetroAchievements))
    } else {
        (None, None)
    }
}

fn remove_managed_cover(path: &Path) {
    if path.starts_with(cover_directory()) {
        let _ = fs::remove_file(path);
    }
}

fn normalize(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_lowercase()
}

pub fn library_path() -> PathBuf {
    persistence::app_directory().join("library.json")
}

pub fn cover_directory() -> PathBuf {
    persistence::app_directory().join("library-covers")
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn test_rom() -> Vec<u8> {
        let mut rom = vec![0; 16 + 0x4000 + 0x2000];
        rom[0..4].copy_from_slice(b"NES\x1a");
        rom[4] = 1;
        rom[5] = 1;
        rom[16..19].copy_from_slice(&[0x4c, 0x00, 0x80]);
        rom[16 + 0x3ffa..16 + 0x4000].copy_from_slice(&[0x00, 0x80, 0x00, 0x80, 0x00, 0x80]);
        rom
    }

    #[test]
    fn scan_marks_invalid_files_and_deduplicates_identical_roms() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("my-own-nes-library-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("game.nes"), test_rom()).unwrap();
        fs::write(dir.join("duplicate.nes"), test_rom()).unwrap();
        fs::write(dir.join("broken.nes"), b"not a ROM").unwrap();

        let mut library = RomLibrary {
            stored: Vec::new(),
            hidden: Vec::new(),
            entries: Vec::new(),
        };
        library.refresh(Some(&dir));
        assert_eq!(library.entries.len(), 2);
        assert_eq!(
            library
                .entries
                .iter()
                .filter(|entry| matches!(&entry.status, EntryStatus::Ready))
                .count(),
            1
        );
        let ready = library
            .entries
            .iter()
            .find(|entry| matches!(&entry.status, EntryStatus::Ready))
            .unwrap();
        assert_eq!(ready.accuracy.as_ref().unwrap().score, 100);
        assert_eq!(ready.accuracy.as_ref().unwrap().rating, "High");
        assert_eq!(ready.accuracy.as_ref().unwrap().mapper_coverage, "Focused");
        assert_eq!(ready.accuracy.as_ref().unwrap().passed, 101);
        assert_eq!(ready.accuracy.as_ref().unwrap().total, 141);
        assert!(
            library
                .entries
                .iter()
                .any(|entry| matches!(&entry.status, EntryStatus::Invalid(_)))
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn refresh_recomputes_accuracy_from_the_current_rom() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "crabnes-accuracy-refresh-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("game.nes");
        let mut rom = test_rom();
        fs::write(&path, &rom).unwrap();

        let mut library = RomLibrary {
            stored: Vec::new(),
            hidden: Vec::new(),
            entries: Vec::new(),
        };
        library.refresh(Some(&dir));
        assert_eq!(library.entries[0].accuracy.as_ref().unwrap().score, 100);

        rom[9] = 1;
        fs::write(&path, rom).unwrap();
        library.refresh(Some(&dir));
        assert_eq!(library.entries[0].accuracy.as_ref().unwrap().score, 92);
        assert!(
            library.entries[0]
                .accuracy
                .as_ref()
                .unwrap()
                .details
                .iter()
                .any(|detail| detail.contains("PAL timing"))
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn nes2_submapper_without_specialized_board_logic_lowers_confidence() {
        let mut rom = vec![0; 16 + 0x8000 + 0x2000];
        rom[0..4].copy_from_slice(b"NES\x1a");
        rom[4] = 2;
        rom[5] = 1;
        rom[6] = 0x20;
        rom[7] = 0x08;
        rom[8] = 0x10;
        rom[16 + 0x7ffa..16 + 0x8000].copy_from_slice(&[0x00, 0x80, 0x00, 0x80, 0x00, 0x80]);
        let nes = Nes::from_ines(&rom).unwrap();
        let estimate = estimate_accuracy(&nes, &rom);
        assert_eq!(estimate.score, 92);
        assert_eq!(estimate.rating, "High");
        assert_eq!(estimate.mapper_coverage, "Focused");
        assert!(
            estimate
                .details
                .iter()
                .any(|detail| detail.contains("Unknown specialized NES 2.0 submapper"))
        );
    }

    #[test]
    fn mapper_coverage_adjusts_the_accuracycoin_weighted_rom_score() {
        let mut rom = vec![0; 16 + 0x20_000 + 0x20_000];
        rom[0..4].copy_from_slice(b"NES\x1a");
        rom[4] = 8;
        rom[5] = 16;
        rom[6] = 0x90;
        rom[16 + 0x1fffa..16 + 0x20_000].copy_from_slice(&[0x00, 0x80, 0x00, 0x80, 0x00, 0x80]);
        let nes = Nes::from_ines(&rom).unwrap();

        let estimate = estimate_accuracy(&nes, &rom);

        assert_eq!(estimate.score, 95);
        assert_eq!(estimate.passed, 101);
        assert_eq!(estimate.total, 141);
        assert_eq!(estimate.rating, "High");
        assert_eq!(estimate.mapper_coverage, "Partial");
        assert!(
            estimate
                .details
                .iter()
                .any(|detail| detail.contains("Mapper 9 coverage: Partial"))
        );
    }

    #[test]
    fn accuracycoin_category_weights_match_the_supplied_test_array() {
        assert_eq!(accuracycoin_penalty(ACCURACYCOIN_OFFICIAL_CPU), 0);
        assert_eq!(accuracycoin_penalty(ACCURACYCOIN_UNOFFICIAL_CPU), 0);
        assert_eq!(accuracycoin_penalty(ACCURACYCOIN_PPU), 11);
        assert_eq!(accuracycoin_penalty(ACCURACYCOIN_APU_DMA), 4);
        assert_eq!(
            ACCURACYCOIN_OFFICIAL_CPU.0
                + ACCURACYCOIN_UNOFFICIAL_CPU.0
                + ACCURACYCOIN_PPU.0
                + ACCURACYCOIN_APU_DMA.0,
            ACCURACYCOIN_PASSED
        );
        assert_eq!(
            ACCURACYCOIN_OFFICIAL_CPU.1
                + ACCURACYCOIN_UNOFFICIAL_CPU.1
                + ACCURACYCOIN_PPU.1
                + ACCURACYCOIN_APU_DMA.1,
            ACCURACYCOIN_TOTAL
        );
    }

    #[test]
    fn reachable_code_scales_only_the_hardware_categories_it_uses() {
        let mut rom = test_rom();
        let program = [
            0xa9, 0x00, // LDA #0
            0x8d, 0x00, 0x20, // STA $2000: control
            0xad, 0x02, 0x20, // LDA $2002: status
            0x8d, 0x05, 0x20, // STA $2005: scrolling
            0x8d, 0x14, 0x40, // STA $4014: sprite DMA/OAM
            0x8d, 0x00, 0x40, // STA $4000: regular APU
            0x8d, 0x15, 0x40, // STA $4015: APU enable
            0x8d, 0x17, 0x40, // STA $4017: frame counter
            0x4c, 0x17, 0x80, // JMP $8017
        ];
        rom[16..16 + program.len()].copy_from_slice(&program);
        let nes = Nes::from_ines(&rom).unwrap();

        let estimate = estimate_accuracy(&nes, &rom);

        assert_eq!(estimate.score, 91);
        assert!(
            estimate
                .details
                .iter()
                .any(|detail| detail.contains("PPU -7 at 65% exposure"))
        );
        assert!(
            estimate
                .details
                .iter()
                .any(|detail| detail.contains("APU/DMA -2 at 50% exposure"))
        );
    }

    #[test]
    fn dmc_code_receives_the_full_apu_dma_risk() {
        let mut rom = test_rom();
        let program = [
            0xa9, 0x00, // LDA #0
            0x8d, 0x10, 0x40, // STA $4010: DMC control
            0x4c, 0x05, 0x80, // JMP $8005
        ];
        rom[16..16 + program.len()].copy_from_slice(&program);
        let nes = Nes::from_ines(&rom).unwrap();

        let estimate = estimate_accuracy(&nes, &rom);

        assert_eq!(estimate.score, 96);
        assert!(
            estimate
                .details
                .iter()
                .any(|detail| detail.contains("APU/DMA -4 at 100% exposure"))
        );
    }

    #[test]
    fn unreachable_data_does_not_inflate_hardware_risk() {
        let mut rom = test_rom();
        rom[16 + 3..16 + 9].copy_from_slice(&[0x8d, 0x00, 0x20, 0x8d, 0x10, 0x40]);
        let nes = Nes::from_ines(&rom).unwrap();

        let estimate = estimate_accuracy(&nes, &rom);

        assert_eq!(estimate.score, 100);
        assert!(
            estimate
                .details
                .iter()
                .any(|detail| detail.contains("PPU -0 at 0% exposure"))
        );
    }

    #[test]
    fn custom_artwork_overrides_retro_artwork() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "crabnes-cover-priority-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        let custom = dir.join("custom.png");
        let retro = dir.join("retro.png");
        fs::write(&custom, b"custom").unwrap();
        fs::write(&retro, b"retro").unwrap();

        assert_eq!(
            effective_cover(Some(custom.clone()), Some(retro.clone())),
            (Some(custom.clone()), Some(CoverSource::Custom))
        );
        fs::remove_file(&custom).unwrap();
        assert_eq!(
            effective_cover(Some(custom), Some(retro.clone())),
            (Some(retro), Some(CoverSource::RetroAchievements))
        );

        let _ = fs::remove_dir_all(dir);
    }
}

use super::*;

pub(super) struct RewindPoint {
    pub(super) compressed_machine: Vec<u8>,
    pub(super) uncompressed_len: usize,
    pub(super) generation: u64,
    pub(super) tas_cursor: usize,
    pub(super) lag_frames: u64,
    pub(super) controller_reads: u64,
}

pub(super) struct RewindCapture {
    pub(super) machine: Vec<u8>,
    pub(super) generation: u64,
    pub(super) tas_cursor: usize,
    pub(super) lag_frames: u64,
    pub(super) controller_reads: u64,
}

impl RewindPoint {
    pub(super) fn compress(capture: RewindCapture) -> Self {
        Self {
            compressed_machine: compress_prepend_size(&capture.machine),
            uncompressed_len: capture.machine.len(),
            generation: capture.generation,
            tas_cursor: capture.tas_cursor,
            lag_frames: capture.lag_frames,
            controller_reads: capture.controller_reads,
        }
    }

    pub(super) fn decompress(&self) -> Result<Vec<u8>, lz4_flex::block::DecompressError> {
        decompress_size_prepended(&self.compressed_machine)
    }
}

pub(super) struct RewindCompressor {
    pub(super) captures: SyncSender<RewindCapture>,
    pub(super) points: Receiver<RewindPoint>,
}

impl RewindCompressor {
    pub(super) fn new() -> Self {
        // A short bounded queue prevents compression from ever building an
        // unbounded latency or memory backlog behind live emulation.
        let (capture_tx, capture_rx) = mpsc::sync_channel::<RewindCapture>(2);
        let (point_tx, point_rx) = mpsc::channel();
        thread::Builder::new()
            .name("rewind-compressor".into())
            .spawn(move || {
                while let Ok(capture) = capture_rx.recv() {
                    if point_tx.send(RewindPoint::compress(capture)).is_err() {
                        break;
                    }
                }
            })
            .expect("could not start rewind compression worker");
        Self {
            captures: capture_tx,
            points: point_rx,
        }
    }

    pub(super) fn submit(&self, capture: RewindCapture) {
        match self.captures.try_send(capture) {
            Ok(()) | Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
        }
    }
}

impl App {
    pub(super) fn collect_compressed_rewind_points(&mut self) {
        loop {
            match self.rewind_compressor.points.try_recv() {
                Ok(point) if point.generation == self.rewind_generation => {
                    self.rewind.push_back(point);
                }
                Ok(_) => {}
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        let interval = self.settings.emulation.rewind_interval_frames.max(1) as usize;
        let native_frames_per_second = self.emulation_frame_rate().ceil() as usize;
        let max = self
            .settings
            .emulation
            .rewind_seconds
            .saturating_mul(native_frames_per_second)
            / interval;
        while self.rewind.len() > max.max(1) {
            self.rewind.pop_front();
        }
    }

    pub(super) fn invalidate_pending_rewind_captures(&mut self) {
        self.rewind_generation = self.rewind_generation.wrapping_add(1);
    }

    pub(super) fn clear_rewind_history(&mut self) {
        self.invalidate_pending_rewind_captures();
        self.rewind.clear();
    }

    pub(super) fn update_continuous_rewind(&mut self, held: bool) {
        let interval = Duration::from_secs_f64(1.0 / REWIND_UPDATES_PER_SECOND);
        let now = Instant::now();
        if held {
            if !self.rewind_active {
                self.rewind_active = true;
                self.resume_after_rewind = !self.paused;
                self.paused = true;
                self.frame_budget = 0.0;
                self.next_rewind_step = now;
                self.invalidate_pending_rewind_captures();
                self.clear_audio_pipeline();
            }
            if now >= self.next_rewind_step {
                self.rewind_step();
                // Advance the deadline instead of resetting it to `now`, so
                // timer jitter cannot accumulate into a lower rewind rate.
                self.next_rewind_step =
                    advance_rewind_deadline(self.next_rewind_step, now, interval);
            }
        } else if self.rewind_active {
            self.rewind_active = false;
            if self.resume_after_rewind && self.powered {
                self.paused = false;
                self.status = "Resumed after rewind".into();
            } else {
                self.status = "Rewind stopped".into();
            }
            self.resume_after_rewind = false;
            self.frame_budget = 0.0;
            self.clear_audio_pipeline();
        }
    }

    pub(super) fn rewind_step(&mut self) {
        if self.play_mode().restricts_assists() {
            self.status = format!("Rewind is disabled in {} mode", self.play_mode().label());
            return;
        }
        if !self.rewind_active {
            self.invalidate_pending_rewind_captures();
        }
        if self.tas.movie.is_some() {
            self.rewind_tas_one_frame();
        } else {
            self.rewind_once();
        }
    }

    pub(super) fn rewind_tas_one_frame(&mut self) {
        if self.tas.cursor == 0 {
            self.status = "TAS is already at frame 0".into();
            return;
        }
        let target = self.tas.cursor - 1;
        let recording = self.tas.recording_context();
        if !self.seek_tas(target) {
            return;
        }
        let removed = if recording && self.tas.resume_recording() {
            // The machine is immediately before `target`; branching here must
            // remove that input and everything after it.
            self.tas.truncate_recording_at(target)
        } else {
            0
        };
        self.follow_tas_cursor();
        self.status = if recording {
            format!("Rewound exactly 1 TAS frame to {target}; removed {removed} future frame(s)")
        } else {
            format!("Rewound exactly 1 TAS frame to {target}")
        };
        self.presented_frames_in_window = self.presented_frames_in_window.wrapping_add(1);
    }

    pub(super) fn rewind_once(&mut self) {
        let Some(point) = self.rewind.pop_back() else {
            self.status = "Rewind buffer is empty".into();
            return;
        };
        let machine = match point.decompress() {
            Ok(machine) => machine,
            Err(error) => {
                self.status = format!("Rewind snapshot is corrupt: {error}");
                return;
            }
        };
        let Some(nes) = self.nes.as_mut() else {
            return;
        };
        if let Err(error) = nes.load_state(&machine) {
            self.status = format!("Rewind restore failed: {error}");
            return;
        }
        let recording_rewind = self.tas.recording_context();
        let removed = if recording_rewind {
            self.tas.truncate_recording_at(point.tas_cursor)
        } else {
            0
        };
        if self.tas.movie.is_some() && self.tas.mode != TasMode::Inactive {
            self.tas.set_cursor_paused(point.tas_cursor);
        } else {
            self.tas.cursor = point.tas_cursor;
        }
        self.lag_frames = point.lag_frames;
        self.last_controller_reads = point.controller_reads;
        self.paused = true;
        self.frame_dirty = true;
        self.presented_frames_in_window = self.presented_frames_in_window.wrapping_add(1);
        if self.tas.movie.is_some() {
            self.follow_tas_cursor();
        }
        self.clear_audio_pipeline();
        self.status = if recording_rewind {
            format!(
                "Rewound to TAS frame {}; removed {removed} future input frame(s)",
                point.tas_cursor
            )
        } else if self.rewind_active {
            "Rewinding…".into()
        } else {
            "Rewound".into()
        };
    }
}

pub(super) fn advance_rewind_deadline(
    deadline: Instant,
    now: Instant,
    interval: Duration,
) -> Instant {
    let next = deadline + interval;
    if now.saturating_duration_since(next) >= interval {
        now + interval
    } else {
        next
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{RewindCapture, RewindPoint, advance_rewind_deadline};

    #[test]
    fn rewind_points_round_trip_through_fast_compression() {
        let machine = (0..256_u16)
            .flat_map(|byte| [byte as u8; 256])
            .collect::<Vec<_>>();
        let point = RewindPoint::compress(RewindCapture {
            machine: machine.clone(),
            generation: 9,
            tas_cursor: 12,
            lag_frames: 3,
            controller_reads: 4,
        });
        assert!(point.compressed_machine.len() < machine.len());
        assert_eq!(point.generation, 9);
        assert_eq!(point.decompress().unwrap(), machine);
    }

    #[test]
    fn rewind_deadline_carries_small_scheduler_delays_without_drifting() {
        let start = Instant::now();
        let interval = Duration::from_millis(10);
        let slightly_late = start + Duration::from_millis(12);
        assert_eq!(
            advance_rewind_deadline(start, slightly_late, interval),
            start + interval
        );

        let badly_late = start + Duration::from_millis(25);
        assert_eq!(
            advance_rewind_deadline(start, badly_late, interval),
            badly_late + interval
        );
    }
}

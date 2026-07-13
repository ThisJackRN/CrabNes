use nes_audio_native::NativeAudio;

const STARTUP_BUFFER_MILLISECONDS: u32 = 40;
const CAPACITY_MILLISECONDS: u32 = 250;

pub struct AudioOutput {
    native: NativeAudio,
    volume: f32,
    muted: bool,
    reference_mastering: bool,
    scratch: Vec<f32>,
}

impl AudioOutput {
    pub fn new(source_rate: u32) -> Result<Self, String> {
        let target_frames = source_rate * STARTUP_BUFFER_MILLISECONDS / 1_000;
        let capacity_frames = source_rate * CAPACITY_MILLISECONDS / 1_000;
        Ok(Self {
            native: NativeAudio::new(source_rate, target_frames, capacity_frames)?,
            volume: 0.75,
            muted: false,
            reference_mastering: true,
            scratch: Vec::with_capacity(1_024),
        })
    }

    pub fn push(&mut self, samples: &[f32]) {
        self.scratch.clear();
        self.scratch.extend(samples.iter().map(|&sample| {
            let sample = if self.reference_mastering {
                master_like_reference(sample)
            } else {
                sample
            };
            if self.muted {
                0.0
            } else {
                sample * self.volume
            }
        }));
        self.native.push(&self.scratch);
    }

    pub fn clear(&self) {
        self.native.clear();
    }

    pub fn queued_samples(&self) -> usize {
        self.native.queued_frames()
    }

    pub fn underflows(&self) -> u32 {
        self.native.underflows()
    }

    pub fn overflows(&self) -> u32 {
        self.native.overflows()
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    pub fn set_reference_mastering(&mut self, enabled: bool) {
        self.reference_mastering = enabled;
    }

    pub fn device_name(&self) -> &str {
        self.native.device_name()
    }

    pub fn device_sample_rate(&self) -> u32 {
        self.native.device_rate()
    }
}

fn master_like_reference(sample: f32) -> f32 {
    const DRIVE: f32 = 3.4;
    const GAIN: f32 = 0.3;
    (sample * DRIVE).tanh() * GAIN
}

#[cfg(test)]
mod tests {
    use super::master_like_reference;

    #[test]
    fn reference_mastering_preserves_small_signals_and_limits_peaks() {
        assert!((master_like_reference(0.01) - 0.01).abs() < 0.001);
        assert!(master_like_reference(1.0) < 0.3);
        assert_eq!(master_like_reference(-1.0), -master_like_reference(1.0));
    }
}

use crate::CPU_CLOCK_HZ;

use super::OUTPUT_SAMPLE_RATE;

#[derive(Clone, Copy)]
pub(super) struct Levels {
    pub pulse_1: u8,
    pub pulse_2: u8,
    pub triangle: u8,
    pub noise: u8,
    pub dmc: u8,
}

pub(super) struct Sampler {
    phase: u32,
    pulse: [f64; 2],
    triangle: f64,
    noise: f64,
    dmc: f64,
    clocks: u32,
    dc_blocker: DcBlocker,
}

impl Default for Sampler {
    fn default() -> Self {
        Self {
            phase: 0,
            pulse: [0.0; 2],
            triangle: 0.0,
            noise: 0.0,
            dmc: 0.0,
            clocks: 0,
            dc_blocker: DcBlocker::default(),
        }
    }
}

impl Sampler {
    pub(super) fn clock(&mut self, levels: Levels) -> Option<f32> {
        self.pulse[0] += f64::from(levels.pulse_1);
        self.pulse[1] += f64::from(levels.pulse_2);
        self.triangle += f64::from(levels.triangle);
        self.noise += f64::from(levels.noise);
        self.dmc += f64::from(levels.dmc);
        self.clocks += 1;

        self.phase += OUTPUT_SAMPLE_RATE;
        if self.phase < CPU_CLOCK_HZ {
            return None;
        }
        self.phase -= CPU_CLOCK_HZ;

        let divisor = f64::from(self.clocks);
        let mixed = nonlinear_mix(
            (self.pulse[0] / divisor) as f32,
            (self.pulse[1] / divisor) as f32,
            (self.triangle / divisor) as f32,
            (self.noise / divisor) as f32,
            (self.dmc / divisor) as f32,
        );
        self.pulse = [0.0; 2];
        self.triangle = 0.0;
        self.noise = 0.0;
        self.dmc = 0.0;
        self.clocks = 0;

        Some((self.dc_blocker.process(mixed) * 2.0).clamp(-1.0, 1.0))
    }
}

fn nonlinear_mix(pulse_1: f32, pulse_2: f32, triangle: f32, noise: f32, dmc: f32) -> f32 {
    let pulse_sum = pulse_1 + pulse_2;
    let pulse = if pulse_sum == 0.0 {
        0.0
    } else {
        95.88 / (8128.0 / pulse_sum + 100.0)
    };

    let tnd_input = triangle / 8227.0 + noise / 12241.0 + dmc / 22638.0;
    let tnd = if tnd_input == 0.0 {
        0.0
    } else {
        159.79 / (1.0 / tnd_input + 100.0)
    };
    pulse + tnd
}

struct DcBlocker {
    previous_input: f32,
    previous_output: f32,
}

impl Default for DcBlocker {
    fn default() -> Self {
        Self {
            previous_input: 0.0,
            previous_output: 0.0,
        }
    }
}

impl DcBlocker {
    fn process(&mut self, input: f32) -> f32 {
        const POLE: f32 = 1.0 - 3.0 / 32_768.0;
        let output = input - self.previous_input + POLE * self.previous_output;
        self.previous_input = input;
        self.previous_output = output;
        output
    }
}

#[cfg(test)]
mod tests {
    use super::nonlinear_mix;

    #[test]
    fn dmc_participates_in_the_nonlinear_tnd_mixer() {
        let silent = nonlinear_mix(0.0, 0.0, 0.0, 0.0, 0.0);
        let dmc = nonlinear_mix(0.0, 0.0, 0.0, 0.0, 64.0);
        assert_eq!(silent, 0.0);
        assert!(dmc > 0.0);
    }
}

use crate::CPU_CLOCK_HZ;
use serde::{Deserialize, Serialize};

use super::OUTPUT_SAMPLE_RATE;

#[derive(Clone, Copy)]
pub(super) struct Levels {
    pub pulse_1: u8,
    pub pulse_2: u8,
    pub triangle: u8,
    pub noise: u8,
    pub dmc: u8,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Sampler {
    phase: u32,
    integrated: f64,
    anti_alias: [LowPass; 2],
    high_pass: [HighPass; 2],
    output_low_pass: LowPass,
}

impl Default for Sampler {
    fn default() -> Self {
        Self {
            phase: 0,
            integrated: 0.0,
            // Mix at the CPU clock before resampling. Two gentle CPU-rate
            // stages reduce ultrasonic timer energy before the exact rational
            // sample-rate conversion below.
            anti_alias: [
                LowPass::new(CPU_CLOCK_HZ as f32, 20_000.0),
                LowPass::new(CPU_CLOCK_HZ as f32, 20_000.0),
            ],
            // NES-style output chain. These cutoff choices follow the common
            // 90 Hz HP -> 440 Hz HP -> 14 kHz LP model also used by TetaNES.
            high_pass: [
                HighPass::new(OUTPUT_SAMPLE_RATE as f32, 90.0),
                HighPass::new(OUTPUT_SAMPLE_RATE as f32, 440.0),
            ],
            output_low_pass: LowPass::new(OUTPUT_SAMPLE_RATE as f32, 14_000.0),
        }
    }
}

impl Sampler {
    pub(super) fn clock(&mut self, levels: Levels) -> Option<f32> {
        // The mixer is nonlinear, so averaging each channel first (the old
        // behavior) is not equivalent and changes transients and timbre.
        let mut mixed = nonlinear_mix(
            f32::from(levels.pulse_1),
            f32::from(levels.pulse_2),
            f32::from(levels.triangle),
            f32::from(levels.noise),
            f32::from(levels.dmc),
        );
        for filter in &mut self.anti_alias {
            mixed = filter.process(mixed);
        }

        // Exact rational, area-preserving CPU-clock -> output-rate conversion.
        // A CPU value is held for one CPU cycle and split at a sample boundary
        // when needed; this produces exactly OUTPUT_SAMPLE_RATE samples per
        // CPU_CLOCK_HZ clocks without frame-rate coupling or repeated samples.
        let mut units_left = OUTPUT_SAMPLE_RATE;
        let mut output = None;
        while units_left > 0 {
            let to_boundary = CPU_CLOCK_HZ - self.phase;
            let units = units_left.min(to_boundary);
            self.integrated += f64::from(mixed) * f64::from(units);
            self.phase += units;
            units_left -= units;

            if self.phase == CPU_CLOCK_HZ {
                let mut sample = (self.integrated / f64::from(CPU_CLOCK_HZ)) as f32;
                self.phase = 0;
                self.integrated = 0.0;
                for filter in &mut self.high_pass {
                    sample = filter.process(sample);
                }
                sample = self.output_low_pass.process(sample);
                output = Some(sample.clamp(-1.0, 1.0));
            }
        }
        output
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

#[derive(Clone, Serialize, Deserialize)]
struct HighPass {
    alpha: f32,
    previous_input: f32,
    previous_output: f32,
}

impl HighPass {
    fn new(sample_rate: f32, cutoff: f32) -> Self {
        let dt = 1.0 / sample_rate;
        let rc = 1.0 / (std::f32::consts::TAU * cutoff);
        Self {
            alpha: rc / (rc + dt),
            previous_input: 0.0,
            previous_output: 0.0,
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        let output = self.alpha * (self.previous_output + input - self.previous_input);
        self.previous_input = input;
        self.previous_output = output;
        output
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct LowPass {
    alpha: f32,
    output: f32,
}

impl LowPass {
    fn new(sample_rate: f32, cutoff: f32) -> Self {
        let dt = 1.0 / sample_rate;
        let rc = 1.0 / (std::f32::consts::TAU * cutoff);
        Self {
            alpha: dt / (rc + dt),
            output: 0.0,
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        self.output += self.alpha * (input - self.output);
        self.output
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

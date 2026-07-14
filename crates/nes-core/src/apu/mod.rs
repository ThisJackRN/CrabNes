//! Ricoh 2A03 audio processing unit.
//!
//! Channel state machines, frame sequencing, nonlinear mixing, sample-rate
//! conversion, and the DMC DMA interface are kept separate so front ends never
//! participate in emulated timing.

mod channels;
mod mixer;

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use channels::{Dmc, Noise, Pulse, Triangle};
use mixer::{Levels, Sampler};

use crate::CPU_CLOCK_HZ;

pub const OUTPUT_SAMPLE_RATE: u32 = 48_000;
const MAX_QUEUED_SAMPLES: usize = OUTPUT_SAMPLE_RATE as usize * 2;
const LENGTH_TABLE: [u8; 32] = [
    10, 254, 20, 2, 40, 4, 80, 6, 160, 8, 60, 10, 14, 12, 26, 14, 12, 16, 24, 18, 48, 20, 96, 22,
    192, 24, 72, 26, 16, 28, 32, 30,
];
const NOISE_PERIODS: [u16; 16] = [
    4, 8, 16, 32, 64, 96, 128, 160, 202, 254, 380, 508, 762, 1016, 2034, 4068,
];
const DMC_PERIODS: [u16; 16] = [
    428, 380, 340, 320, 286, 254, 226, 214, 190, 160, 142, 128, 106, 84, 72, 54,
];

#[derive(Clone, Copy, Debug)]
pub struct ApuState {
    pub pulse_periods: [u16; 2],
    pub pulse_frequencies_hz: [f32; 2],
    pub pulse_levels: [u8; 2],
    pub triangle_period: u16,
    pub triangle_frequency_hz: f32,
    pub triangle_level: u8,
    pub noise_period: u16,
    pub noise_level: u8,
    pub dmc_period: u16,
    pub dmc_level: u8,
    pub frame_five_step: bool,
    pub queued_samples: usize,
    pub channel_output_enabled: [bool; 5],
    pub dropped_samples: u64,
}

/// Debug-only output gates. Disabling a channel here never changes its
/// emulated registers, counters, DMA, or IRQ behavior; it only affects mixing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApuChannel {
    Pulse1 = 0,
    Pulse2 = 1,
    Triangle = 2,
    Noise = 3,
    Dmc = 4,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Apu {
    pulse: [Pulse; 2],
    triangle: Triangle,
    noise: Noise,
    dmc: Dmc,
    frame_counter: FrameCounter,
    cycles: u64,
    sampler: Sampler,
    samples: VecDeque<f32>,
    channel_output_enabled: [bool; 5],
    dropped_samples: u64,
}

impl Default for Apu {
    fn default() -> Self {
        Self {
            pulse: [Pulse::new(true), Pulse::new(false)],
            triangle: Triangle::default(),
            noise: Noise::default(),
            dmc: Dmc::default(),
            frame_counter: FrameCounter::default(),
            cycles: 0,
            sampler: Sampler::default(),
            samples: VecDeque::with_capacity(MAX_QUEUED_SAMPLES),
            channel_output_enabled: [true; 5],
            dropped_samples: 0,
        }
    }
}

impl Apu {
    pub fn reset(&mut self) {
        let channel_output_enabled = self.channel_output_enabled;
        *self = Self::default();
        self.channel_output_enabled = channel_output_enabled;
    }

    pub fn write(&mut self, address: u16, value: u8) {
        match address {
            0x4000..=0x4003 => self.pulse[0].write((address - 0x4000) as u8, value),
            0x4004..=0x4007 => self.pulse[1].write((address - 0x4004) as u8, value),
            0x4008 => self.triangle.write_control(value),
            0x400a => self.triangle.write_timer_low(value),
            0x400b => self.triangle.write_timer_high(value),
            0x400c => self.noise.write_control(value),
            0x400e => self.noise.write_period(value),
            0x400f => self.noise.write_length(value),
            0x4010 => self.dmc.write_control(value),
            0x4011 => self.dmc.write_direct_load(value),
            0x4012 => self.dmc.write_sample_address(value),
            0x4013 => self.dmc.write_sample_length(value),
            0x4015 => {
                self.pulse[0].set_enabled(value & 0x01 != 0);
                self.pulse[1].set_enabled(value & 0x02 != 0);
                self.triangle.set_enabled(value & 0x04 != 0);
                self.noise.set_enabled(value & 0x08 != 0);
                self.dmc.set_enabled(value & 0x10 != 0, self.cycles);
            }
            0x4017 => self.frame_counter.write(value, self.cycles),
            _ => {}
        }
    }

    pub fn read_status(&mut self) -> u8 {
        let status = u8::from(self.pulse[0].length() > 0)
            | (u8::from(self.pulse[1].length() > 0) << 1)
            | (u8::from(self.triangle.length() > 0) << 2)
            | (u8::from(self.noise.length() > 0) << 3)
            | (u8::from(self.dmc.active()) << 4)
            | (u8::from(self.frame_counter.irq_flag) << 6)
            | (u8::from(self.dmc.irq_flag()) << 7);
        self.frame_counter.irq_flag = false;
        status
    }

    pub fn irq_pending(&self) -> bool {
        self.frame_counter.irq_flag || self.dmc.irq_flag()
    }

    pub fn clock(&mut self) {
        self.cycles = self.cycles.wrapping_add(1);

        self.triangle.clock_timer();
        self.noise.clock_timer();
        self.dmc.clock_timer();
        if self.cycles.is_multiple_of(2) {
            self.pulse[0].clock_timer();
            self.pulse[1].clock_timer();
        }

        let events = self.frame_counter.clock();
        if events.quarter {
            self.clock_quarter_frame();
        }
        if events.half {
            self.clock_half_frame();
        }
        self.pulse[0].apply_length_pending();
        self.pulse[1].apply_length_pending();
        self.triangle.apply_length_pending();
        self.noise.apply_length_pending();

        let levels = self.levels();
        if let Some(sample) = self.sampler.clock(levels) {
            if self.samples.len() == MAX_QUEUED_SAMPLES {
                self.samples.pop_front();
                self.dropped_samples = self.dropped_samples.saturating_add(1);
            }
            self.samples.push_back(sample);
        }
    }

    pub fn take_dmc_dma_request(&mut self) -> Option<u16> {
        self.dmc.take_dma_request()
    }

    pub fn supply_dmc_sample(&mut self, value: u8) {
        self.dmc.supply_sample(value);
    }

    pub fn cycles(&self) -> u64 {
        self.cycles
    }

    pub fn drain_samples(&mut self, destination: &mut Vec<f32>) {
        destination.extend(self.samples.drain(..));
    }

    pub(crate) fn clear_samples(&mut self) {
        self.samples.clear();
    }

    pub const fn sample_rate(&self) -> u32 {
        OUTPUT_SAMPLE_RATE
    }

    pub fn set_channel_output_enabled(&mut self, channel: ApuChannel, enabled: bool) {
        self.channel_output_enabled[channel as usize] = enabled;
    }

    pub fn channel_output_enabled(&self, channel: ApuChannel) -> bool {
        self.channel_output_enabled[channel as usize]
    }

    pub fn state(&self) -> ApuState {
        let pulse_periods = [self.pulse[0].timer(), self.pulse[1].timer()];
        let pulse_frequencies_hz = pulse_periods.map(|period| {
            if period < 8 {
                0.0
            } else {
                CPU_CLOCK_HZ as f32 / (16.0 * (f32::from(period) + 1.0))
            }
        });
        let triangle_period = self.triangle.timer();
        ApuState {
            pulse_periods,
            pulse_frequencies_hz,
            pulse_levels: [self.pulse[0].output(), self.pulse[1].output()],
            triangle_period,
            triangle_frequency_hz: CPU_CLOCK_HZ as f32
                / (32.0 * (f32::from(triangle_period) + 1.0)),
            triangle_level: self.triangle.output(),
            noise_period: self.noise.period(),
            noise_level: self.noise.output(),
            dmc_period: self.dmc.rate_period(),
            dmc_level: self.dmc.output(),
            frame_five_step: self.frame_counter.five_step,
            queued_samples: self.samples.len(),
            channel_output_enabled: self.channel_output_enabled,
            dropped_samples: self.dropped_samples,
        }
    }

    fn levels(&self) -> Levels {
        Levels {
            pulse_1: self.pulse[0].output() * u8::from(self.channel_output_enabled[0]),
            pulse_2: self.pulse[1].output() * u8::from(self.channel_output_enabled[1]),
            triangle: self.triangle.output() * u8::from(self.channel_output_enabled[2]),
            noise: self.noise.output() * u8::from(self.channel_output_enabled[3]),
            dmc: self.dmc.output() * u8::from(self.channel_output_enabled[4]),
        }
    }

    fn clock_quarter_frame(&mut self) {
        self.pulse[0].clock_quarter();
        self.pulse[1].clock_quarter();
        self.noise.clock_quarter();
        self.triangle.clock_quarter();
    }

    fn clock_half_frame(&mut self) {
        self.pulse[0].clock_half();
        self.pulse[1].clock_half();
        self.triangle.clock_half();
        self.noise.clock_half();
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct FrameCounter {
    cycle: u32,
    step: usize,
    five_step: bool,
    pending_five_step: bool,
    reset_delay: u8,
    irq_inhibit: bool,
    irq_flag: bool,
    block_counter: u8,
}

#[derive(Default, Debug, Eq, PartialEq)]
struct FrameEvents {
    quarter: bool,
    half: bool,
}

impl FrameCounter {
    // Adapted from TetaNES's CPU-cycle frame sequencer (Copyright 2021
    // Luke Petherbridge; MIT/Apache-2.0). See THIRD_PARTY_NOTICES.md.
    const FOUR_STEP_CYCLES: [u32; 6] = [7_457, 14_913, 22_371, 29_828, 29_829, 29_830];
    const FIVE_STEP_CYCLES: [u32; 6] = [7_457, 14_913, 22_371, 29_829, 37_281, 37_282];

    fn write(&mut self, value: u8, cpu_cycles: u64) {
        self.pending_five_step = value & 0x80 != 0;
        self.irq_inhibit = value & 0x40 != 0;
        if self.irq_inhibit {
            self.irq_flag = false;
        }
        self.reset_delay = 3 + (cpu_cycles as u8 & 1);
    }

    fn clock(&mut self) -> FrameEvents {
        self.cycle += 1;
        let mut events = FrameEvents::default();
        let step_cycles = if self.five_step {
            Self::FIVE_STEP_CYCLES
        } else {
            Self::FOUR_STEP_CYCLES
        };

        if self.cycle == step_cycles[self.step] {
            if !self.five_step && self.step >= 3 && !self.irq_inhibit {
                // The four-step sequencer asserts its IRQ across the final
                // three CPU clocks, matching the hardware-visible race window.
                self.irq_flag = true;
            }
            if self.block_counter == 0 {
                match self.step {
                    0 | 2 => events.quarter = true,
                    1 | 4 => {
                        events.quarter = true;
                        events.half = true;
                    }
                    _ => {}
                }
                if events.quarter {
                    self.block_counter = 2;
                }
            }
            self.step += 1;
            if self.step == step_cycles.len() {
                self.step = 0;
                self.cycle = 0;
            }
        }

        if self.reset_delay > 0 {
            self.reset_delay -= 1;
            if self.reset_delay == 0 {
                self.five_step = self.pending_five_step;
                self.step = 0;
                self.cycle = 0;
                if self.five_step && self.block_counter == 0 {
                    events.quarter = true;
                    events.half = true;
                    self.block_counter = 2;
                }
            }
        }
        if self.block_counter > 0 {
            self.block_counter -= 1;
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn produces_samples_at_48khz() {
        let mut apu = Apu::default();
        for _ in 0..CPU_CLOCK_HZ {
            apu.clock();
        }
        let mut samples = Vec::new();
        apu.drain_samples(&mut samples);
        assert_eq!(samples.len(), OUTPUT_SAMPLE_RATE as usize);
        assert!(samples.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn status_tracks_enabled_length_counters() {
        let mut apu = Apu::default();
        apu.write(0x4015, 1);
        apu.write(0x4003, 0xf8);
        apu.clock();
        assert_eq!(apu.read_status() & 1, 1);
        apu.write(0x4015, 0);
        assert_eq!(apu.read_status() & 1, 0);
    }

    #[test]
    fn noise_timer_uses_cpu_clock_periods() {
        let mut noise = Noise::default();
        noise.prepare_timer_test(0);
        let initial = noise.shift_register();
        for _ in 0..3 {
            noise.clock_timer();
            assert_eq!(noise.shift_register(), initial);
        }
        noise.clock_timer();
        assert_ne!(noise.shift_register(), initial);
    }

    #[test]
    fn triangle_holds_its_dac_level_when_stopped() {
        let mut triangle = Triangle::default();
        triangle.set_test_sequence(7);
        assert_eq!(triangle.output(), 8);
        triangle.set_enabled(true);
        assert_eq!(triangle.output(), 8);
    }

    #[test]
    fn frame_counter_write_takes_effect_after_three_or_four_clocks() {
        let mut apu = Apu::default();
        apu.write(0x4017, 0x80);
        assert!(!apu.frame_counter.five_step);
        apu.clock();
        apu.clock();
        assert!(!apu.frame_counter.five_step);
        apu.clock();
        assert!(apu.frame_counter.five_step);
        assert_eq!(apu.frame_counter.cycle, 0);
    }

    #[test]
    fn output_filter_removes_steady_dac_bias() {
        let mut apu = Apu::default();
        for _ in 0..CPU_CLOCK_HZ * 2 {
            apu.clock();
        }
        let mut samples = Vec::new();
        apu.drain_samples(&mut samples);
        // The triangle DAC powers up at a non-zero level, so the high-pass
        // chain correctly produces a short startup transient. Its settled tail
        // must contain no audible DC bias.
        assert!(
            samples[samples.len() / 2..]
                .iter()
                .all(|sample| sample.abs() < 0.001)
        );
    }

    #[test]
    fn pulse_timer_produces_the_expected_ntsc_pitch() {
        let mut apu = Apu::default();
        apu.pulse[0].set_test_timer(253);
        let mut previous_sequence = apu.pulse[0].sequence();
        let mut transitions = 0;
        let mut cpu_clocks = 0;
        while transitions < 8 {
            apu.clock();
            cpu_clocks += 1;
            let sequence = apu.pulse[0].sequence();
            if sequence != previous_sequence {
                transitions += 1;
                previous_sequence = sequence;
            }
        }
        assert_eq!(cpu_clocks, 16 * (253 + 1));
    }

    #[test]
    fn enabling_dmc_requests_the_programmed_sample_address() {
        let mut apu = Apu::default();
        apu.write(0x4012, 0x20);
        apu.write(0x4013, 0x01);
        apu.write(0x4015, 0x10);
        assert_eq!(apu.take_dmc_dma_request(), None);
        apu.clock();
        apu.clock();
        assert_eq!(apu.take_dmc_dma_request(), Some(0xc800));
        apu.supply_dmc_sample(0xff);
        assert_eq!(apu.read_status() & 0x10, 0x10);
    }

    #[test]
    fn four_step_frame_counter_uses_cpu_cycle_landmarks() {
        let mut counter = FrameCounter::default();
        let mut quarter_cycles = Vec::new();
        let mut half_cycles = Vec::new();
        for cycle in 1..=29_830 {
            let events = counter.clock();
            if events.quarter {
                quarter_cycles.push(cycle);
            }
            if events.half {
                half_cycles.push(cycle);
            }
            if cycle < 29_828 {
                assert!(!counter.irq_flag);
            }
        }
        assert_eq!(quarter_cycles, [7_457, 14_913, 22_371, 29_829]);
        assert_eq!(half_cycles, [14_913, 29_829]);
        assert!(counter.irq_flag);
        assert_eq!(counter.cycle, 0);
    }

    #[test]
    fn five_step_frame_counter_has_no_irq_and_clocks_on_write() {
        let mut counter = FrameCounter::default();
        counter.write(0x80, 0);
        assert_eq!(counter.clock(), FrameEvents::default());
        assert_eq!(counter.clock(), FrameEvents::default());
        let immediate = counter.clock();
        assert!(immediate.quarter && immediate.half);
        for _ in 0..37_282 {
            counter.clock();
        }
        assert!(!counter.irq_flag);
    }

    #[test]
    fn pulse_one_negate_shift_zero_uses_ones_complement_target() {
        let mut pulse_one = Pulse::new(true);
        pulse_one.set_enabled(true);
        pulse_one.write(0, 0xdf);
        pulse_one.write(1, 0x08);
        pulse_one.write(2, 100);
        pulse_one.write(3, 0);
        pulse_one.apply_length_pending();

        let mut pulse_two = Pulse::new(false);
        pulse_two.set_enabled(true);
        pulse_two.write(0, 0xdf);
        pulse_two.write(1, 0x08);
        pulse_two.write(2, 100);
        pulse_two.write(3, 0);
        pulse_two.apply_length_pending();

        assert_eq!(pulse_one.output(), 0);
        assert_eq!(pulse_two.output(), 15);
    }

    #[test]
    fn dmc_last_fetch_loops_or_raises_irq() {
        let mut irq_dmc = Dmc::default();
        irq_dmc.write_control(0x80);
        irq_dmc.set_enabled(true, 0);
        irq_dmc.supply_sample(0xaa);
        assert!(irq_dmc.irq_flag());
        assert!(!irq_dmc.active());

        let mut looping_dmc = Dmc::default();
        looping_dmc.write_control(0xc0);
        looping_dmc.set_enabled(true, 0);
        looping_dmc.supply_sample(0xaa);
        assert!(!looping_dmc.irq_flag());
        assert!(looping_dmc.active());
    }
}

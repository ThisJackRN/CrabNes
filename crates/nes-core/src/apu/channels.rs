use super::{DMC_PERIODS, LENGTH_TABLE, NOISE_PERIODS};

const DUTY_TABLE: [[u8; 8]; 4] = [
    [0, 1, 0, 0, 0, 0, 0, 0],
    [0, 1, 1, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 1, 0, 0, 0],
    [1, 0, 0, 1, 1, 1, 1, 1],
];
const TRIANGLE_TABLE: [u8; 32] = [
    15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
    13, 14, 15,
];

pub(super) struct Pulse {
    enabled: bool,
    first_channel: bool,
    duty: u8,
    sequence: u8,
    timer: u16,
    timer_counter: u16,
    length: u8,
    envelope: Envelope,
    sweep_enabled: bool,
    sweep_period: u8,
    sweep_negate: bool,
    sweep_shift: u8,
    sweep_divider: u8,
    sweep_reload: bool,
}

impl Pulse {
    pub(super) const fn new(first_channel: bool) -> Self {
        Self {
            enabled: false,
            first_channel,
            duty: 0,
            sequence: 0,
            timer: 0,
            timer_counter: 0,
            length: 0,
            envelope: Envelope::new(),
            sweep_enabled: false,
            sweep_period: 0,
            sweep_negate: false,
            sweep_shift: 0,
            sweep_divider: 0,
            sweep_reload: false,
        }
    }

    pub(super) fn write(&mut self, register: u8, value: u8) {
        match register {
            0 => {
                self.duty = value >> 6;
                self.envelope.write(value);
            }
            1 => {
                self.sweep_enabled = value & 0x80 != 0;
                self.sweep_period = (value >> 4) & 7;
                self.sweep_negate = value & 0x08 != 0;
                self.sweep_shift = value & 7;
                self.sweep_reload = true;
            }
            2 => self.timer = (self.timer & 0x0700) | u16::from(value),
            3 => {
                self.timer = (self.timer & 0x00ff) | ((u16::from(value) & 7) << 8);
                if self.enabled {
                    self.length = LENGTH_TABLE[(value >> 3) as usize];
                }
                self.sequence = 0;
                self.envelope.start = true;
            }
            _ => unreachable!(),
        }
    }

    pub(super) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.length = 0;
        }
    }

    pub(super) fn clock_timer(&mut self) {
        if self.timer_counter == 0 {
            self.timer_counter = self.timer;
            self.sequence = (self.sequence + 1) & 7;
        } else {
            self.timer_counter -= 1;
        }
    }

    pub(super) fn clock_quarter(&mut self) {
        self.envelope.clock();
    }

    pub(super) fn clock_half(&mut self) {
        if !self.envelope.loop_flag && self.length > 0 {
            self.length -= 1;
        }

        if self.sweep_divider == 0 {
            if self.sweep_enabled && self.sweep_shift > 0 {
                let target = self.sweep_target();
                if self.timer >= 8 && target <= 0x07ff {
                    self.timer = target;
                }
            }
            self.sweep_divider = self.sweep_period;
        } else {
            self.sweep_divider -= 1;
        }
        if self.sweep_reload {
            self.sweep_reload = false;
            self.sweep_divider = self.sweep_period;
        }
    }

    fn sweep_target(&self) -> u16 {
        if self.sweep_shift == 0 && self.sweep_negate {
            return self.timer;
        }
        let change = self.timer >> self.sweep_shift;
        if self.sweep_negate {
            self.timer
                .wrapping_sub(change)
                .wrapping_sub(u16::from(self.first_channel))
        } else {
            self.timer.wrapping_add(change)
        }
    }

    pub(super) fn output(&self) -> u8 {
        let muted = !self.enabled
            || self.length == 0
            || self.timer < 8
            || self.sweep_target() > 0x07ff
            || DUTY_TABLE[self.duty as usize][self.sequence as usize] == 0;
        if muted { 0 } else { self.envelope.volume() }
    }

    pub(super) fn length(&self) -> u8 {
        self.length
    }

    pub(super) fn timer(&self) -> u16 {
        self.timer
    }

    #[cfg(test)]
    pub(super) fn set_test_timer(&mut self, timer: u16) {
        self.timer = timer;
        self.timer_counter = timer;
    }

    #[cfg(test)]
    pub(super) fn sequence(&self) -> u8 {
        self.sequence
    }
}

#[derive(Default)]
pub(super) struct Triangle {
    enabled: bool,
    control: bool,
    linear_reload: u8,
    linear_counter: u8,
    linear_reload_flag: bool,
    timer: u16,
    timer_counter: u16,
    sequence: u8,
    length: u8,
}

impl Triangle {
    pub(super) fn write_control(&mut self, value: u8) {
        self.control = value & 0x80 != 0;
        self.linear_reload = value & 0x7f;
    }

    pub(super) fn write_timer_low(&mut self, value: u8) {
        self.timer = (self.timer & 0x0700) | u16::from(value);
    }

    pub(super) fn write_timer_high(&mut self, value: u8) {
        self.timer = (self.timer & 0x00ff) | ((u16::from(value) & 7) << 8);
        if self.enabled {
            self.length = LENGTH_TABLE[(value >> 3) as usize];
        }
        self.linear_reload_flag = true;
    }

    pub(super) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.length = 0;
        }
    }

    pub(super) fn clock_timer(&mut self) {
        if self.timer_counter == 0 {
            self.timer_counter = self.timer;
            if self.length > 0 && self.linear_counter > 0 && self.timer >= 2 {
                self.sequence = (self.sequence + 1) & 31;
            }
        } else {
            self.timer_counter -= 1;
        }
    }

    pub(super) fn clock_quarter(&mut self) {
        if self.linear_reload_flag {
            self.linear_counter = self.linear_reload;
        } else if self.linear_counter > 0 {
            self.linear_counter -= 1;
        }
        if !self.control {
            self.linear_reload_flag = false;
        }
    }

    pub(super) fn clock_half(&mut self) {
        if !self.control && self.length > 0 {
            self.length -= 1;
        }
    }

    pub(super) fn output(&self) -> u8 {
        TRIANGLE_TABLE[self.sequence as usize]
    }

    pub(super) fn length(&self) -> u8 {
        self.length
    }

    pub(super) fn timer(&self) -> u16 {
        self.timer
    }

    #[cfg(test)]
    pub(super) fn set_test_sequence(&mut self, sequence: u8) {
        self.sequence = sequence & 31;
    }
}

pub(super) struct Noise {
    enabled: bool,
    mode: bool,
    period_index: u8,
    timer_counter: u16,
    shift_register: u16,
    length: u8,
    envelope: Envelope,
}

impl Default for Noise {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: false,
            period_index: 0,
            timer_counter: 0,
            shift_register: 1,
            length: 0,
            envelope: Envelope::new(),
        }
    }
}

impl Noise {
    pub(super) fn write_control(&mut self, value: u8) {
        self.envelope.write(value);
    }

    pub(super) fn write_period(&mut self, value: u8) {
        self.mode = value & 0x80 != 0;
        self.period_index = value & 0x0f;
    }

    pub(super) fn write_length(&mut self, value: u8) {
        if self.enabled {
            self.length = LENGTH_TABLE[(value >> 3) as usize];
        }
        self.envelope.start = true;
    }

    pub(super) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.length = 0;
        }
    }

    pub(super) fn clock_timer(&mut self) {
        if self.timer_counter == 0 {
            self.timer_counter = self.period() - 1;
            let tap = if self.mode { 6 } else { 1 };
            let feedback = (self.shift_register & 1) ^ ((self.shift_register >> tap) & 1);
            self.shift_register = (self.shift_register >> 1) | (feedback << 14);
        } else {
            self.timer_counter -= 1;
        }
    }

    pub(super) fn clock_quarter(&mut self) {
        self.envelope.clock();
    }

    pub(super) fn clock_half(&mut self) {
        if !self.envelope.loop_flag && self.length > 0 {
            self.length -= 1;
        }
    }

    pub(super) fn output(&self) -> u8 {
        if !self.enabled || self.length == 0 || self.shift_register & 1 != 0 {
            0
        } else {
            self.envelope.volume()
        }
    }

    pub(super) fn length(&self) -> u8 {
        self.length
    }

    pub(super) fn period(&self) -> u16 {
        NOISE_PERIODS[self.period_index as usize]
    }

    #[cfg(test)]
    pub(super) fn prepare_timer_test(&mut self, period_index: u8) {
        self.period_index = period_index;
        self.timer_counter = self.period() - 1;
    }

    #[cfg(test)]
    pub(super) fn shift_register(&self) -> u16 {
        self.shift_register
    }
}

pub(super) struct Dmc {
    enabled: bool,
    irq_enabled: bool,
    loop_flag: bool,
    irq_flag: bool,
    rate_index: u8,
    timer_counter: u16,
    output_level: u8,
    sample_address: u16,
    sample_length: u16,
    current_address: u16,
    bytes_remaining: u16,
    sample_buffer: Option<u8>,
    shift_register: u8,
    bits_remaining: u8,
    silence: bool,
    dma_pending: bool,
}

impl Default for Dmc {
    fn default() -> Self {
        Self {
            enabled: false,
            irq_enabled: false,
            loop_flag: false,
            irq_flag: false,
            rate_index: 0,
            timer_counter: DMC_PERIODS[0] - 1,
            output_level: 0,
            sample_address: 0xc000,
            sample_length: 1,
            current_address: 0xc000,
            bytes_remaining: 0,
            sample_buffer: None,
            shift_register: 0,
            bits_remaining: 8,
            silence: true,
            dma_pending: false,
        }
    }
}

impl Dmc {
    pub(super) fn write_control(&mut self, value: u8) {
        self.irq_enabled = value & 0x80 != 0;
        self.loop_flag = value & 0x40 != 0;
        self.rate_index = value & 0x0f;
        if !self.irq_enabled {
            self.irq_flag = false;
        }
    }

    pub(super) fn write_direct_load(&mut self, value: u8) {
        self.output_level = value & 0x7f;
    }

    pub(super) fn write_sample_address(&mut self, value: u8) {
        self.sample_address = 0xc000 | (u16::from(value) << 6);
    }

    pub(super) fn write_sample_length(&mut self, value: u8) {
        self.sample_length = (u16::from(value) << 4) | 1;
    }

    pub(super) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.irq_flag = false;
        if enabled {
            if self.bytes_remaining == 0 {
                self.restart_sample();
            }
        } else {
            self.bytes_remaining = 0;
            self.dma_pending = false;
        }
    }

    fn restart_sample(&mut self) {
        self.current_address = self.sample_address;
        self.bytes_remaining = self.sample_length;
        self.request_dma_if_needed();
    }

    fn request_dma_if_needed(&mut self) {
        if self.enabled && self.sample_buffer.is_none() && self.bytes_remaining > 0 {
            self.dma_pending = true;
        }
    }

    pub(super) fn clock_timer(&mut self) {
        if self.timer_counter == 0 {
            self.timer_counter = DMC_PERIODS[self.rate_index as usize] - 1;
            self.clock_output();
        } else {
            self.timer_counter -= 1;
        }
        self.request_dma_if_needed();
    }

    fn clock_output(&mut self) {
        if !self.silence {
            if self.shift_register & 1 != 0 {
                if self.output_level <= 125 {
                    self.output_level += 2;
                }
            } else if self.output_level >= 2 {
                self.output_level -= 2;
            }
        }
        self.shift_register >>= 1;
        self.bits_remaining -= 1;
        if self.bits_remaining == 0 {
            self.bits_remaining = 8;
            if let Some(sample) = self.sample_buffer.take() {
                self.shift_register = sample;
                self.silence = false;
            } else {
                self.silence = true;
            }
        }
    }

    pub(super) fn take_dma_request(&mut self) -> Option<u16> {
        if self.dma_pending {
            self.dma_pending = false;
            Some(self.current_address)
        } else {
            None
        }
    }

    pub(super) fn supply_sample(&mut self, value: u8) {
        if self.bytes_remaining == 0 {
            return;
        }
        self.sample_buffer = Some(value);
        self.current_address = if self.current_address == 0xffff {
            0x8000
        } else {
            self.current_address + 1
        };
        self.bytes_remaining -= 1;
        if self.bytes_remaining == 0 {
            if self.loop_flag {
                self.restart_sample();
            } else if self.irq_enabled {
                self.irq_flag = true;
            }
        }
    }

    pub(super) fn output(&self) -> u8 {
        self.output_level
    }

    pub(super) fn active(&self) -> bool {
        self.bytes_remaining > 0
    }

    pub(super) fn irq_flag(&self) -> bool {
        self.irq_flag
    }

    pub(super) fn rate_period(&self) -> u16 {
        DMC_PERIODS[self.rate_index as usize]
    }
}

struct Envelope {
    loop_flag: bool,
    constant: bool,
    period: u8,
    start: bool,
    divider: u8,
    decay: u8,
}

impl Envelope {
    const fn new() -> Self {
        Self {
            loop_flag: false,
            constant: false,
            period: 0,
            start: false,
            divider: 0,
            decay: 0,
        }
    }

    fn write(&mut self, value: u8) {
        self.loop_flag = value & 0x20 != 0;
        self.constant = value & 0x10 != 0;
        self.period = value & 0x0f;
    }

    fn clock(&mut self) {
        if self.start {
            self.start = false;
            self.decay = 15;
            self.divider = self.period;
        } else if self.divider == 0 {
            self.divider = self.period;
            if self.decay > 0 {
                self.decay -= 1;
            } else if self.loop_flag {
                self.decay = 15;
            }
        } else {
            self.divider -= 1;
        }
    }

    fn volume(&self) -> u8 {
        if self.constant {
            self.period
        } else {
            self.decay
        }
    }
}

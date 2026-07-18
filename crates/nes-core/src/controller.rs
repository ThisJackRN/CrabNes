#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Button {
    A = 0,
    B = 1,
    Select = 2,
    Start = 3,
    Up = 4,
    Down = 5,
    Left = 6,
    Right = 7,
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Controller {
    buttons: u8,
    shift: u8,
    strobe: bool,
    total_reads: u64,
    #[serde(default)]
    coin: bool,
}

impl Controller {
    pub(crate) fn import_fceux_serial_state(&mut self, buttons: u8, read_bit: u8, strobe: bool) {
        self.buttons = buttons;
        self.shift = if read_bit >= 8 {
            0xff
        } else if read_bit == 0 {
            buttons
        } else {
            (buttons >> read_bit) | (!0u8 << (8 - read_bit))
        };
        self.strobe = strobe;
        self.total_reads = 0;
        self.coin = false;
    }

    pub fn set_button(&mut self, button: Button, pressed: bool) {
        let mask = 1 << button as u8;
        if pressed {
            self.buttons |= mask;
        } else {
            self.buttons &= !mask;
        }
        if self.strobe {
            self.shift = self.buttons;
        }
    }

    pub fn button(&self, button: Button) -> bool {
        self.buttons & (1 << button as u8) != 0
    }

    pub fn set_coin(&mut self, inserted: bool) {
        self.coin = inserted;
    }

    pub fn coin(&self) -> bool {
        self.coin
    }

    pub(crate) fn write_strobe(&mut self, value: u8, sample_latch: bool) {
        let new_strobe = value & 1 != 0;
        if sample_latch && (self.strobe || new_strobe) {
            self.shift = self.buttons;
        }
        self.strobe = new_strobe;
    }

    /// Read the serial joypad bit.
    ///
    /// `clock` is true when this access should advance the shift register.
    /// Hardware clocks on every read slot — DMA cycles overlapping a
    /// $4016/$4017 read therefore multi-shift the pad, the read corruption
    /// games like SMB3 mitigate with re-read loops. The bus passes
    /// `clock: false` only in its FCEUX-compatibility model, which folds
    /// contiguous same-port reads into a single shift for movie playback.
    pub(crate) fn read(&mut self, clock: bool) -> u8 {
        self.total_reads = self.total_reads.wrapping_add(1);
        if self.strobe {
            self.shift = self.buttons;
        }
        let value = self.shift & 1;
        if clock && !self.strobe {
            self.shift = (self.shift >> 1) | 0x80;
        }
        value
    }

    pub fn total_reads(&self) -> u64 {
        self.total_reads
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shifts_buttons_in_nes_order() {
        let mut pad = Controller::default();
        pad.set_button(Button::A, true);
        pad.set_button(Button::Start, true);
        pad.write_strobe(1, true);
        pad.write_strobe(0, true);
        let bits: Vec<_> = (0..8).map(|_| pad.read(true) & 1).collect();
        assert_eq!(bits, vec![1, 0, 0, 1, 0, 0, 0, 0]);
    }

    #[test]
    fn contiguous_oe_without_clock_does_not_shift() {
        let mut pad = Controller::default();
        pad.set_button(Button::A, true);
        pad.set_button(Button::B, true);
        pad.write_strobe(1, true);
        pad.write_strobe(0, true);
        assert_eq!(pad.read(true) & 1, 1); // A
        assert_eq!(pad.read(false) & 1, 1); // still A — OE held, no clock
        assert_eq!(pad.read(true) & 1, 1); // B (clocked)
    }
}

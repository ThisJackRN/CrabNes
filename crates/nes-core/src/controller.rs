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
}

impl Controller {
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

    pub(crate) fn write_strobe(&mut self, value: u8) {
        let new_strobe = value & 1 != 0;
        if self.strobe || new_strobe {
            self.shift = self.buttons;
        }
        self.strobe = new_strobe;
    }

    pub(crate) fn read(&mut self) -> u8 {
        self.total_reads = self.total_reads.wrapping_add(1);
        if self.strobe {
            self.shift = self.buttons;
        }
        let value = self.shift & 1;
        if !self.strobe {
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
        pad.write_strobe(1);
        pad.write_strobe(0);
        let bits: Vec<_> = (0..8).map(|_| pad.read() & 1).collect();
        assert_eq!(bits, vec![1, 0, 0, 1, 0, 0, 0, 0]);
    }
}

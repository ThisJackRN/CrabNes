use serde::{Deserialize, Serialize};

use super::{
    CartridgeError, Mirroring,
    mapper::{Mapper, MapperSnapshot, bank_offset, load_trainer},
};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Fme7Snapshot {
    command: u8,
    chr_banks: [u8; 8],
    prg_banks: [u8; 4],
    mirroring: Mirroring,
    irq_counter: u16,
    irq_counter_enabled: bool,
    irq_enabled: bool,
    irq_pending: bool,
    audio_select: u8,
    audio_registers: [u8; 16],
    tone_counters: [u16; 3],
    tone_phases: [bool; 3],
    audio_divider: u8,
    noise_counter: u8,
    noise_lfsr: u32,
    noise_phase: bool,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
}

pub struct Fme7 {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    chr_is_ram: bool,
    command: u8,
    chr_banks: [u8; 8],
    prg_banks: [u8; 4],
    mirroring: Mirroring,
    irq_counter: u16,
    irq_counter_enabled: bool,
    irq_enabled: bool,
    irq_pending: bool,
    audio_select: u8,
    audio_registers: [u8; 16],
    tone_counters: [u16; 3],
    tone_phases: [bool; 3],
    audio_divider: u8,
    noise_counter: u8,
    noise_lfsr: u32,
    noise_phase: bool,
}

impl Fme7 {
    pub fn new(
        prg_rom: Vec<u8>,
        chr: Vec<u8>,
        prg_ram_size: usize,
        trainer: Option<&[u8]>,
    ) -> Result<Self, CartridgeError> {
        if prg_rom.len() < 0x8000 || !prg_rom.len().is_multiple_of(0x2000) {
            return Err(CartridgeError::InvalidMapperRomSize {
                mapper: 69,
                kind: "PRG ROM",
                size: prg_rom.len(),
            });
        }
        if !chr.is_empty() && !chr.len().is_multiple_of(0x0400) {
            return Err(CartridgeError::InvalidMapperRomSize {
                mapper: 69,
                kind: "CHR",
                size: chr.len(),
            });
        }
        let chr_is_ram = chr.is_empty();
        let mut prg_ram = vec![0; prg_ram_size.max(0x2000)];
        load_trainer(&mut prg_ram, trainer);
        Ok(Self {
            prg_rom,
            prg_ram,
            chr: if chr_is_ram { vec![0; 0x2000] } else { chr },
            chr_is_ram,
            command: 0,
            chr_banks: [0; 8],
            prg_banks: [0; 4],
            mirroring: Mirroring::Vertical,
            irq_counter: 0,
            irq_counter_enabled: false,
            irq_enabled: false,
            irq_pending: false,
            audio_select: 0,
            audio_registers: [0; 16],
            tone_counters: [0; 3],
            tone_phases: [false; 3],
            audio_divider: 0,
            noise_counter: 0,
            noise_lfsr: 1,
            noise_phase: false,
        })
    }

    fn apply_parameter(&mut self, value: u8) {
        match self.command {
            0..=7 => self.chr_banks[usize::from(self.command)] = value,
            8..=11 => self.prg_banks[usize::from(self.command - 8)] = value,
            12 => {
                self.mirroring = match value & 3 {
                    0 => Mirroring::Vertical,
                    1 => Mirroring::Horizontal,
                    2 => Mirroring::SingleScreenLower,
                    _ => Mirroring::SingleScreenUpper,
                }
            }
            13 => {
                self.irq_counter_enabled = value & 0x80 != 0;
                self.irq_enabled = value & 1 != 0;
                self.irq_pending = false;
            }
            14 => self.irq_counter = (self.irq_counter & 0xff00) | u16::from(value),
            15 => self.irq_counter = (self.irq_counter & 0x00ff) | (u16::from(value) << 8),
            _ => {}
        }
    }

    fn clock_audio_divider(&mut self) {
        for channel in 0..3 {
            let period = (u16::from(self.audio_registers[channel * 2 + 1] & 0x0f) << 8)
                | u16::from(self.audio_registers[channel * 2]);
            self.tone_counters[channel] += 1;
            if self.tone_counters[channel] >= period.max(1) {
                self.tone_counters[channel] = 0;
                self.tone_phases[channel] = !self.tone_phases[channel];
            }
        }
        self.noise_counter += 1;
        if self.noise_counter >= (self.audio_registers[6] & 0x1f).max(1) {
            self.noise_counter = 0;
            let feedback = ((self.noise_lfsr >> 16) ^ (self.noise_lfsr >> 13)) & 1;
            self.noise_lfsr = ((self.noise_lfsr << 1) | feedback) & 0x1ffff;
            self.noise_phase = self.noise_lfsr & 1 != 0;
        }
    }

    fn audio_level(&self) -> f32 {
        let mut output = 0.0;
        for channel in 0..3 {
            let mixer = self.audio_registers[7];
            let tone = mixer & (1 << channel) != 0 || self.tone_phases[channel];
            let noise = mixer & (1 << (channel + 3)) != 0 || self.noise_phase;
            if tone && noise {
                let volume = self.audio_registers[8 + channel] & 0x0f;
                if volume != 0 {
                    output += 10_f32.powf((f32::from(volume) - 15.0) * 3.0 / 20.0) * 0.10;
                }
            }
        }
        output
    }
}

impl Mapper for Fme7 {
    fn cpu_read(&mut self, address: u16) -> Option<u8> {
        self.cpu_peek(address)
    }
    fn cpu_peek(&self, address: u16) -> Option<u8> {
        match address {
            0x6000..=0x7fff => {
                let register = self.prg_banks[0];
                if register & 0x40 != 0 {
                    (register & 0x80 != 0)
                        .then(|| self.prg_ram[usize::from(address - 0x6000) % self.prg_ram.len()])
                } else {
                    Some(
                        self.prg_rom[bank_offset(
                            usize::from(register & 0x3f),
                            0x2000,
                            usize::from(address & 0x1fff),
                            self.prg_rom.len(),
                        )],
                    )
                }
            }
            0x8000..=0xdfff => {
                let slot = usize::from((address - 0x8000) / 0x2000) + 1;
                Some(
                    self.prg_rom[bank_offset(
                        usize::from(self.prg_banks[slot] & 0x3f),
                        0x2000,
                        usize::from(address & 0x1fff),
                        self.prg_rom.len(),
                    )],
                )
            }
            0xe000..=0xffff => {
                Some(self.prg_rom[self.prg_rom.len() - 0x2000 + usize::from(address - 0xe000)])
            }
            _ => None,
        }
    }
    fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        match address {
            0x6000..=0x7fff => {
                let r = self.prg_banks[0];
                if r & 0xc0 == 0xc0 {
                    let o = usize::from(address - 0x6000) % self.prg_ram.len();
                    self.prg_ram[o] = value;
                }
                true
            }
            0x8000..=0x9fff => {
                self.command = value & 0x0f;
                true
            }
            0xa000..=0xbfff => {
                self.apply_parameter(value);
                true
            }
            0xc000..=0xdfff => {
                self.audio_select = if value & 0xf0 == 0 {
                    value & 0x0f
                } else {
                    0xff
                };
                true
            }
            0xe000..=0xffff => {
                if let Some(register) = self.audio_registers.get_mut(usize::from(self.audio_select))
                {
                    *register = value;
                }
                true
            }
            _ => false,
        }
    }
    fn ppu_read(&mut self, address: u16) -> Option<u8> {
        (address <= 0x1fff).then(|| {
            let slot = usize::from(address / 0x0400);
            self.chr[bank_offset(
                usize::from(self.chr_banks[slot]),
                0x0400,
                usize::from(address & 0x03ff),
                self.chr.len(),
            )]
        })
    }
    fn ppu_write(&mut self, address: u16, value: u8) -> bool {
        if address > 0x1fff {
            return false;
        }
        if self.chr_is_ram {
            let slot = usize::from(address / 0x0400);
            let o = bank_offset(
                usize::from(self.chr_banks[slot]),
                0x0400,
                usize::from(address & 0x03ff),
                self.chr.len(),
            );
            self.chr[o] = value;
        }
        true
    }
    fn mirroring(&self) -> Option<Mirroring> {
        Some(self.mirroring)
    }
    fn clock_cpu(&mut self) {
        if self.irq_counter_enabled {
            let old = self.irq_counter;
            self.irq_counter = self.irq_counter.wrapping_sub(1);
            if old == 0 && self.irq_enabled {
                self.irq_pending = true;
            }
        }
        self.audio_divider += 1;
        if self.audio_divider >= 16 {
            self.audio_divider = 0;
            self.clock_audio_divider();
        }
    }
    fn irq_pending(&self) -> bool {
        self.irq_pending
    }
    fn expansion_audio(&self) -> f32 {
        self.audio_level()
    }
    fn reset(&mut self) {
        self.irq_pending = false;
        self.irq_enabled = false;
        self.irq_counter_enabled = false;
    }
    fn battery_ram(&self) -> Option<&[u8]> {
        Some(&self.prg_ram)
    }
    fn load_battery_ram(&mut self, data: &[u8]) {
        let count = data.len().min(self.prg_ram.len());
        self.prg_ram[..count].copy_from_slice(&data[..count]);
    }
    fn snapshot(&self) -> MapperSnapshot {
        MapperSnapshot::Fme7(Fme7Snapshot {
            command: self.command,
            chr_banks: self.chr_banks,
            prg_banks: self.prg_banks,
            mirroring: self.mirroring,
            irq_counter: self.irq_counter,
            irq_counter_enabled: self.irq_counter_enabled,
            irq_enabled: self.irq_enabled,
            irq_pending: self.irq_pending,
            audio_select: self.audio_select,
            audio_registers: self.audio_registers,
            tone_counters: self.tone_counters,
            tone_phases: self.tone_phases,
            audio_divider: self.audio_divider,
            noise_counter: self.noise_counter,
            noise_lfsr: self.noise_lfsr,
            noise_phase: self.noise_phase,
            prg_ram: self.prg_ram.clone(),
            chr: self.chr.clone(),
        })
    }
    fn restore_snapshot(&mut self, snapshot: &MapperSnapshot) -> bool {
        let MapperSnapshot::Fme7(s) = snapshot else {
            return false;
        };
        if s.prg_ram.len() != self.prg_ram.len() || s.chr.len() != self.chr.len() {
            return false;
        }
        self.command = s.command;
        self.chr_banks = s.chr_banks;
        self.prg_banks = s.prg_banks;
        self.mirroring = s.mirroring;
        self.irq_counter = s.irq_counter;
        self.irq_counter_enabled = s.irq_counter_enabled;
        self.irq_enabled = s.irq_enabled;
        self.irq_pending = s.irq_pending;
        self.audio_select = s.audio_select;
        self.audio_registers = s.audio_registers;
        self.tone_counters = s.tone_counters;
        self.tone_phases = s.tone_phases;
        self.audio_divider = s.audio_divider;
        self.noise_counter = s.noise_counter;
        self.noise_lfsr = s.noise_lfsr;
        self.noise_phase = s.noise_phase;
        self.prg_ram.copy_from_slice(&s.prg_ram);
        if self.chr_is_ram {
            self.chr.copy_from_slice(&s.chr)
        }
        true
    }
    fn prg_rom(&self) -> &[u8] {
        &self.prg_rom
    }
    fn chr(&self) -> &[u8] {
        &self.chr
    }
    fn chr_is_writable(&self) -> bool {
        self.chr_is_ram
    }
    fn debug_write_chr(&mut self, offset: usize, value: u8) -> bool {
        self.chr_is_ram
            && self.chr.get_mut(offset).is_some_and(|b| {
                *b = value;
                true
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn switches_banks_and_counts_cpu_irq() {
        let mut prg = vec![0; 8 * 0x2000];
        for b in 0..8 {
            prg[b * 0x2000] = b as u8
        }
        let mut m = Fme7::new(prg, vec![], 0x2000, None).unwrap();
        m.cpu_write(0x8000, 9);
        m.cpu_write(0xa000, 3);
        assert_eq!(m.cpu_read(0x8000), Some(3));
        m.cpu_write(0x8000, 14);
        m.cpu_write(0xa000, 1);
        m.cpu_write(0x8000, 15);
        m.cpu_write(0xa000, 0);
        m.cpu_write(0x8000, 13);
        m.cpu_write(0xa000, 0x81);
        m.clock_cpu();
        assert!(!m.irq_pending());
        m.clock_cpu();
        assert!(m.irq_pending());
        m.cpu_write(0xc000, 7);
        m.cpu_write(0xe000, 0x09);
        m.cpu_write(0xc000, 8);
        m.cpu_write(0xe000, 15);
        assert!(m.expansion_audio() > 0.0);
    }
}

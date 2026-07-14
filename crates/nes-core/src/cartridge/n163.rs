use super::{
    CartridgeError,
    mapper::{Mapper, MapperSnapshot, bank_offset, load_trainer},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct N163Snapshot {
    submapper: u8,
    chr_banks: [u8; 12],
    prg_banks: [u8; 3],
    chr_ram_disable: u8,
    irq_counter: u16,
    irq_enabled: bool,
    audio_ram: Vec<u8>,
    audio_address: u8,
    audio_increment: bool,
    audio_divider: u8,
    audio_channel: u8,
    audio_outputs: [i16; 8],
    sound_disabled: bool,
    ram_protect: u8,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
}
pub struct N163 {
    submapper: u8,
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    chr_is_ram: bool,
    chr_banks: [u8; 12],
    prg_banks: [u8; 3],
    chr_ram_disable: u8,
    irq_counter: u16,
    irq_enabled: bool,
    audio_ram: Vec<u8>,
    audio_address: u8,
    audio_increment: bool,
    audio_divider: u8,
    audio_channel: u8,
    audio_outputs: [i16; 8],
    sound_disabled: bool,
    ram_protect: u8,
}
impl N163 {
    pub fn new(
        submapper: u8,
        prg_rom: Vec<u8>,
        chr: Vec<u8>,
        prg_ram_size: usize,
        trainer: Option<&[u8]>,
    ) -> Result<Self, CartridgeError> {
        if prg_rom.len() < 0x8000 || !prg_rom.len().is_multiple_of(0x2000) {
            return Err(CartridgeError::InvalidMapperRomSize {
                mapper: 19,
                kind: "PRG ROM",
                size: prg_rom.len(),
            });
        }
        if !chr.is_empty() && !chr.len().is_multiple_of(0x0400) {
            return Err(CartridgeError::InvalidMapperRomSize {
                mapper: 19,
                kind: "CHR",
                size: chr.len(),
            });
        }
        let chr_is_ram = chr.is_empty();
        let mut prg_ram = vec![0; prg_ram_size.max(0x2000)];
        load_trainer(&mut prg_ram, trainer);
        Ok(Self {
            submapper,
            prg_rom,
            prg_ram,
            chr: if chr_is_ram { vec![0; 0x2000] } else { chr },
            chr_is_ram,
            chr_banks: [0; 12],
            prg_banks: [0; 3],
            chr_ram_disable: 0,
            irq_counter: 0,
            irq_enabled: false,
            audio_ram: vec![0; 128],
            audio_address: 0,
            audio_increment: false,
            audio_divider: 0,
            audio_channel: 0,
            audio_outputs: [0; 8],
            sound_disabled: false,
            ram_protect: 0,
        })
    }
    fn audio_port_read(&mut self) -> u8 {
        let v = self.audio_ram[usize::from(self.audio_address)];
        if self.audio_increment {
            self.audio_address = (self.audio_address + 1) & 0x7f
        }
        v
    }
    fn audio_port_write(&mut self, v: u8) {
        self.audio_ram[usize::from(self.audio_address)] = v;
        if self.audio_increment {
            self.audio_address = (self.audio_address + 1) & 0x7f
        }
    }
    fn update_audio(&mut self) {
        let count = ((self.audio_ram[0x7f] >> 4) & 7) + 1;
        self.audio_channel %= count;
        let logical = 7 - self.audio_channel;
        let base = 0x40 + usize::from(logical) * 8;
        let freq = u32::from(self.audio_ram[base])
            | (u32::from(self.audio_ram[base + 2]) << 8)
            | (u32::from(self.audio_ram[base + 4] & 3) << 16);
        let length = 256 - u32::from(self.audio_ram[base + 4] & 0xfc);
        let mut phase = u32::from(self.audio_ram[base + 1])
            | (u32::from(self.audio_ram[base + 3]) << 8)
            | (u32::from(self.audio_ram[base + 5]) << 16);
        phase = (phase + freq) % (length << 16);
        self.audio_ram[base + 1] = phase as u8;
        self.audio_ram[base + 3] = (phase >> 8) as u8;
        self.audio_ram[base + 5] = (phase >> 16) as u8;
        let sample_index = ((phase >> 16) + u32::from(self.audio_ram[base + 6])) & 0xff;
        let packed = self.audio_ram[(sample_index / 2) as usize & 0x7f];
        let sample = if sample_index & 1 == 0 {
            packed & 15
        } else {
            packed >> 4
        };
        let volume = self.audio_ram[base + 7] & 15;
        self.audio_outputs[usize::from(logical)] = (i16::from(sample) - 8) * i16::from(volume);
        self.audio_channel = (self.audio_channel + 1) % count
    }
    fn ciram_bank(&self, slot: usize) -> Option<usize> {
        let bank = self.chr_banks[8 + slot];
        (bank >= 0xe0).then_some(usize::from(bank & 1))
    }
}
impl Mapper for N163 {
    fn cpu_read(&mut self, a: u16) -> Option<u8> {
        match a {
            0x4800..=0x4fff => Some(self.audio_port_read()),
            0x5000..=0x57ff => Some(self.irq_counter as u8),
            0x5800..=0x5fff => {
                Some(((self.irq_counter >> 8) as u8 & 0x7f) | (u8::from(self.irq_enabled) << 7))
            }
            _ => self.cpu_peek(a),
        }
    }
    fn cpu_peek(&self, a: u16) -> Option<u8> {
        match a {
            0x4800..=0x4fff => Some(self.audio_ram[usize::from(self.audio_address)]),
            0x5000..=0x57ff => Some(self.irq_counter as u8),
            0x5800..=0x5fff => {
                Some(((self.irq_counter >> 8) as u8 & 0x7f) | (u8::from(self.irq_enabled) << 7))
            }
            0x6000..=0x7fff => Some(self.prg_ram[usize::from(a - 0x6000) % self.prg_ram.len()]),
            0x8000..=0xdfff => {
                let s = usize::from((a - 0x8000) / 0x2000);
                Some(
                    self.prg_rom[bank_offset(
                        usize::from(self.prg_banks[s]),
                        0x2000,
                        usize::from(a & 0x1fff),
                        self.prg_rom.len(),
                    )],
                )
            }
            0xe000..=0xffff => {
                Some(self.prg_rom[self.prg_rom.len() - 0x2000 + usize::from(a - 0xe000)])
            }
            _ => None,
        }
    }
    fn cpu_write(&mut self, a: u16, v: u8) -> bool {
        match a {
            0x4800..=0x4fff => {
                self.audio_port_write(v);
                true
            }
            0x5000..=0x57ff => {
                self.irq_counter = (self.irq_counter & 0x7f00) | u16::from(v);
                true
            }
            0x5800..=0x5fff => {
                self.irq_counter = (self.irq_counter & 0x00ff) | (u16::from(v & 0x7f) << 8);
                self.irq_enabled = v & 0x80 != 0;
                true
            }
            0x6000..=0x7fff => {
                let window = usize::from((a - 0x6000) / 0x0800);
                if self.ram_protect & 0xf0 == 0x40 && self.ram_protect & (1 << window) == 0 {
                    let o = usize::from(a - 0x6000) % self.prg_ram.len();
                    self.prg_ram[o] = v
                }
                true
            }
            0x8000..=0xdfff => {
                let s = usize::from((a - 0x8000) / 0x0800);
                self.chr_banks[s] = v;
                true
            }
            0xe000..=0xe7ff => {
                self.prg_banks[0] = v & 0x3f;
                self.sound_disabled = v & 0x40 != 0;
                true
            }
            0xe800..=0xefff => {
                self.prg_banks[1] = v & 0x3f;
                self.chr_ram_disable = v & 0xc0;
                true
            }
            0xf000..=0xf7ff => {
                self.prg_banks[2] = v & 0x3f;
                true
            }
            0xf800..=0xffff => {
                self.audio_address = v & 0x7f;
                self.audio_increment = v & 0x80 != 0;
                self.ram_protect = v;
                true
            }
            _ => false,
        }
    }
    fn ppu_read(&mut self, a: u16) -> Option<u8> {
        if a > 0x1fff {
            return None;
        }
        let s = usize::from(a / 0x0400);
        let b = self.chr_banks[s];
        if b >= 0xe0
            && ((s < 4 && self.chr_ram_disable & 0x40 == 0)
                || (s >= 4 && self.chr_ram_disable & 0x80 == 0))
        {
            None
        } else {
            Some(
                self.chr[bank_offset(
                    usize::from(b),
                    0x0400,
                    usize::from(a & 0x03ff),
                    self.chr.len(),
                )],
            )
        }
    }
    fn ppu_write(&mut self, a: u16, v: u8) -> bool {
        if a > 0x1fff {
            return false;
        }
        let s = usize::from(a / 0x0400);
        let b = self.chr_banks[s];
        if b >= 0xe0
            && ((s < 4 && self.chr_ram_disable & 0x40 == 0)
                || (s >= 4 && self.chr_ram_disable & 0x80 == 0))
        {
            return false;
        }
        if self.chr_is_ram {
            let o = bank_offset(
                usize::from(b),
                0x0400,
                usize::from(a & 0x03ff),
                self.chr.len(),
            );
            self.chr[o] = v
        }
        true
    }
    fn nametable_read(&mut self, a: u16) -> Option<u8> {
        let s = usize::from((a & 0x0fff) / 0x0400);
        let b = self.chr_banks[8 + s];
        (b < 0xe0).then(|| {
            self.chr[bank_offset(
                usize::from(b),
                0x0400,
                usize::from(a & 0x03ff),
                self.chr.len(),
            )]
        })
    }
    fn nametable_write(&mut self, a: u16, v: u8) -> bool {
        let s = usize::from((a & 0x0fff) / 0x0400);
        let b = self.chr_banks[8 + s];
        if b < 0xe0 && self.chr_is_ram {
            let o = bank_offset(
                usize::from(b),
                0x0400,
                usize::from(a & 0x03ff),
                self.chr.len(),
            );
            self.chr[o] = v;
            true
        } else {
            b < 0xe0
        }
    }
    fn nametable_ciram_index(&self, a: u16) -> Option<usize> {
        let s = usize::from((a & 0x0fff) / 0x0400);
        self.ciram_bank(s)
            .map(|b| b * 0x400 + usize::from(a & 0x03ff))
    }
    fn clock_cpu(&mut self) {
        if self.irq_enabled && self.irq_counter < 0x7fff {
            self.irq_counter += 1
        }
        if !self.sound_disabled {
            self.audio_divider += 1;
            if self.audio_divider >= 15 {
                self.audio_divider = 0;
                self.update_audio()
            }
        }
    }
    fn irq_pending(&self) -> bool {
        self.irq_enabled && self.irq_counter == 0x7fff
    }
    fn expansion_audio(&self) -> f32 {
        if self.sound_disabled || self.submapper == 2 {
            return 0.0;
        }
        let count = f32::from(((self.audio_ram[0x7f] >> 4) & 7) + 1);
        let level: i16 = self.audio_outputs.iter().sum();
        f32::from(level) / (120.0 * count)
            * match self.submapper {
                4 => 0.16,
                5 => 0.2,
                _ => 0.12,
            }
    }
    fn reset(&mut self) {
        self.irq_enabled = false;
        self.irq_counter = 0
    }
    fn battery_ram(&self) -> Option<&[u8]> {
        Some(&self.prg_ram)
    }
    fn load_battery_ram(&mut self, d: &[u8]) {
        let c = d.len().min(self.prg_ram.len());
        self.prg_ram[..c].copy_from_slice(&d[..c])
    }
    fn snapshot(&self) -> MapperSnapshot {
        MapperSnapshot::N163(N163Snapshot {
            submapper: self.submapper,
            chr_banks: self.chr_banks,
            prg_banks: self.prg_banks,
            chr_ram_disable: self.chr_ram_disable,
            irq_counter: self.irq_counter,
            irq_enabled: self.irq_enabled,
            audio_ram: self.audio_ram.clone(),
            audio_address: self.audio_address,
            audio_increment: self.audio_increment,
            audio_divider: self.audio_divider,
            audio_channel: self.audio_channel,
            audio_outputs: self.audio_outputs,
            sound_disabled: self.sound_disabled,
            ram_protect: self.ram_protect,
            prg_ram: self.prg_ram.clone(),
            chr: self.chr.clone(),
        })
    }
    fn restore_snapshot(&mut self, s: &MapperSnapshot) -> bool {
        let MapperSnapshot::N163(s) = s else {
            return false;
        };
        if s.submapper != self.submapper
            || s.prg_ram.len() != self.prg_ram.len()
            || s.chr.len() != self.chr.len()
            || s.audio_ram.len() != 128
        {
            return false;
        }
        self.chr_banks = s.chr_banks;
        self.prg_banks = s.prg_banks;
        self.chr_ram_disable = s.chr_ram_disable;
        self.irq_counter = s.irq_counter;
        self.irq_enabled = s.irq_enabled;
        self.audio_ram.clone_from(&s.audio_ram);
        self.audio_address = s.audio_address;
        self.audio_increment = s.audio_increment;
        self.audio_divider = s.audio_divider;
        self.audio_channel = s.audio_channel;
        self.audio_outputs = s.audio_outputs;
        self.sound_disabled = s.sound_disabled;
        self.ram_protect = s.ram_protect;
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
    fn debug_write_chr(&mut self, o: usize, v: u8) -> bool {
        self.chr_is_ram
            && self.chr.get_mut(o).is_some_and(|b| {
                *b = v;
                true
            })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn audio_port_banks_and_irq_work() {
        let mut p = vec![0; 8 * 0x2000];
        for b in 0..8 {
            p[b * 0x2000] = b as u8
        }
        let mut m = N163::new(3, p, vec![], 0x2000, None).unwrap();
        m.cpu_write(0xe000, 3);
        assert_eq!(m.cpu_read(0x8000), Some(3));
        m.cpu_write(0xf800, 0x80);
        m.cpu_write(0x4800, 0x55);
        assert_eq!(m.audio_ram[0], 0x55);
        m.cpu_write(0x5000, 0xfe);
        m.cpu_write(0x5800, 0xff);
        m.clock_cpu();
        assert!(m.irq_pending());
    }
}

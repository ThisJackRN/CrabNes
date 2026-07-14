use serde::{Deserialize, Serialize};

use super::{
    CartridgeError, Mirroring,
    mapper::{Mapper, MapperSnapshot, bank_offset, load_trainer},
};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct VrcSnapshot {
    mapper_id: u16,
    submapper: u8,
    prg_banks: [u8; 2],
    chr_banks: [u16; 8],
    mirroring: Mirroring,
    swap_mode: bool,
    wram_enabled: bool,
    vrc2_latch: u8,
    irq_latch: u8,
    irq_counter: u8,
    irq_prescaler: i16,
    irq_mode_cycle: bool,
    irq_enable_after_ack: bool,
    irq_enabled: bool,
    irq_pending: bool,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
}

pub struct Vrc {
    mapper_id: u16,
    submapper: u8,
    is_vrc2: bool,
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    chr_is_ram: bool,
    prg_banks: [u8; 2],
    chr_banks: [u16; 8],
    mirroring: Mirroring,
    swap_mode: bool,
    wram_enabled: bool,
    vrc2_latch: u8,
    irq_latch: u8,
    irq_counter: u8,
    irq_prescaler: i16,
    irq_mode_cycle: bool,
    irq_enable_after_ack: bool,
    irq_enabled: bool,
    irq_pending: bool,
}

impl Vrc {
    pub fn new(
        mapper_id: u16,
        submapper: u8,
        prg_rom: Vec<u8>,
        chr: Vec<u8>,
        prg_ram_size: usize,
        trainer: Option<&[u8]>,
    ) -> Result<Self, CartridgeError> {
        if prg_rom.len() < 0x8000 || !prg_rom.len().is_multiple_of(0x2000) {
            return Err(CartridgeError::InvalidMapperRomSize {
                mapper: mapper_id,
                kind: "PRG ROM",
                size: prg_rom.len(),
            });
        }
        if !chr.is_empty() && !chr.len().is_multiple_of(0x0400) {
            return Err(CartridgeError::InvalidMapperRomSize {
                mapper: mapper_id,
                kind: "CHR",
                size: chr.len(),
            });
        }
        let chr_is_ram = chr.is_empty();
        let mut prg_ram = vec![0; prg_ram_size.max(0x2000)];
        load_trainer(&mut prg_ram, trainer);
        let is_vrc2 = mapper_id == 22 || matches!((mapper_id, submapper), (23, 3) | (25, 3));
        Ok(Self {
            mapper_id,
            submapper,
            is_vrc2,
            prg_rom,
            prg_ram,
            chr: if chr_is_ram { vec![0; 0x2000] } else { chr },
            chr_is_ram,
            prg_banks: [0, 1],
            chr_banks: [0; 8],
            mirroring: Mirroring::Vertical,
            swap_mode: false,
            wram_enabled: !is_vrc2,
            vrc2_latch: 0,
            irq_latch: 0,
            irq_counter: 0,
            irq_prescaler: 341,
            irq_mode_cycle: false,
            irq_enable_after_ack: false,
            irq_enabled: false,
            irq_pending: false,
        })
    }

    fn register_index(&self, address: u16) -> usize {
        let direct = usize::from(address & 3);
        let swap = usize::from(((address & 1) << 1) | ((address >> 1) & 1));
        match (self.mapper_id, self.submapper) {
            (21, 1) => usize::from((address >> 1) & 3),
            (21, 2) => usize::from((address >> 6) & 3),
            (21, _) => {
                if address & 0x00c0 != 0 {
                    usize::from((address >> 6) & 3)
                } else {
                    usize::from((address >> 1) & 3)
                }
            }
            (22, _) => swap,
            (23, 2) => usize::from((address >> 2) & 3),
            (23, _) => {
                if self.submapper == 0 && address & 0x000c != 0 {
                    usize::from((address >> 2) & 3)
                } else {
                    direct
                }
            }
            (25, 2) => usize::from(((address >> 2) & 2) | ((address >> 3) & 1)),
            (25, _) => swap,
            _ => direct,
        }
    }
    fn prg_bank(&self, slot: usize) -> usize {
        let last = self.prg_rom.len() / 0x2000 - 1;
        let second = last - 1;
        match slot {
            0 if !self.swap_mode => usize::from(self.prg_banks[0]),
            0 => second,
            1 => usize::from(self.prg_banks[1]),
            2 if !self.swap_mode => second,
            2 => usize::from(self.prg_banks[0]),
            _ => last,
        }
    }
    fn set_chr_nibble(&mut self, group: usize, index: usize, value: u8) {
        let bank = group * 2 + index / 2;
        if index & 1 == 0 {
            self.chr_banks[bank] = (self.chr_banks[bank] & 0x1f0) | u16::from(value & 0x0f)
        } else {
            let high = if self.is_vrc2 {
                value & 0x0f
            } else {
                value & 0x1f
            };
            self.chr_banks[bank] = (self.chr_banks[bank] & 0x00f) | (u16::from(high) << 4)
        }
    }
    fn clock_irq_counter(&mut self) {
        if self.irq_counter == 0xff {
            self.irq_counter = self.irq_latch;
            self.irq_pending = true
        } else {
            self.irq_counter = self.irq_counter.wrapping_add(1)
        }
    }
}

impl Mapper for Vrc {
    fn cpu_read(&mut self, a: u16) -> Option<u8> {
        self.cpu_peek(a)
    }
    fn cpu_peek(&self, a: u16) -> Option<u8> {
        match a {
            0x6000..=0x7fff if self.is_vrc2 => Some(
                (self.vrc2_latch & 1)
                    | (self.prg_ram[usize::from(a - 0x6000) % self.prg_ram.len()] & 0xfe),
            ),
            0x6000..=0x7fff if self.wram_enabled => {
                Some(self.prg_ram[usize::from(a - 0x6000) % self.prg_ram.len()])
            }
            0x8000..=0xffff => {
                let s = usize::from((a - 0x8000) / 0x2000);
                Some(
                    self.prg_rom[bank_offset(
                        self.prg_bank(s),
                        0x2000,
                        usize::from(a & 0x1fff),
                        self.prg_rom.len(),
                    )],
                )
            }
            _ => None,
        }
    }
    fn cpu_write(&mut self, a: u16, v: u8) -> bool {
        match a {
            0x6000..=0x7fff => {
                if self.is_vrc2 {
                    self.vrc2_latch = v & 1
                } else if self.wram_enabled {
                    let o = usize::from(a - 0x6000) % self.prg_ram.len();
                    self.prg_ram[o] = v
                }
                true
            }
            0x8000..=0x8fff => {
                self.prg_banks[0] = v & 0x1f;
                true
            }
            0x9000..=0x9fff => {
                let i = self.register_index(a);
                if i == 0 {
                    self.mirroring = match v & 3 {
                        0 => Mirroring::Vertical,
                        1 => Mirroring::Horizontal,
                        2 => Mirroring::SingleScreenLower,
                        _ => Mirroring::SingleScreenUpper,
                    }
                } else if i == 2 && !self.is_vrc2 {
                    self.wram_enabled = v & 1 != 0;
                    self.swap_mode = v & 2 != 0
                }
                true
            }
            0xa000..=0xafff => {
                self.prg_banks[1] = v & 0x1f;
                true
            }
            0xb000..=0xefff => {
                let group = usize::from((a >> 12) - 0x0b);
                let i = self.register_index(a);
                self.set_chr_nibble(group, i, v);
                true
            }
            0xf000..=0xffff if !self.is_vrc2 => {
                match self.register_index(a) {
                    0 => self.irq_latch = (self.irq_latch & 0xf0) | (v & 0x0f),
                    1 => self.irq_latch = (self.irq_latch & 0x0f) | ((v & 0x0f) << 4),
                    2 => {
                        self.irq_enable_after_ack = v & 1 != 0;
                        self.irq_enabled = v & 2 != 0;
                        self.irq_mode_cycle = v & 4 != 0;
                        self.irq_pending = false;
                        self.irq_prescaler = 341;
                        if self.irq_enabled {
                            self.irq_counter = self.irq_latch
                        }
                    }
                    _ => {
                        self.irq_pending = false;
                        self.irq_enabled = self.irq_enable_after_ack
                    }
                }
                true
            }
            _ => false,
        }
    }
    fn ppu_read(&mut self, a: u16) -> Option<u8> {
        (a <= 0x1fff).then(|| {
            let s = usize::from(a / 0x0400);
            let mut b = usize::from(self.chr_banks[s]);
            if self.mapper_id == 22 {
                b >>= 1
            }
            self.chr[bank_offset(b, 0x0400, usize::from(a & 0x03ff), self.chr.len())]
        })
    }
    fn ppu_write(&mut self, a: u16, v: u8) -> bool {
        if a > 0x1fff {
            return false;
        }
        if self.chr_is_ram {
            let s = usize::from(a / 0x0400);
            let mut b = usize::from(self.chr_banks[s]);
            if self.mapper_id == 22 {
                b >>= 1
            }
            let o = bank_offset(b, 0x0400, usize::from(a & 0x03ff), self.chr.len());
            self.chr[o] = v
        }
        true
    }
    fn mirroring(&self) -> Option<Mirroring> {
        Some(self.mirroring)
    }
    fn clock_cpu(&mut self) {
        if !self.irq_enabled {
            return;
        }
        if self.irq_mode_cycle {
            self.clock_irq_counter()
        } else {
            self.irq_prescaler -= 3;
            if self.irq_prescaler <= 0 {
                self.irq_prescaler += 341;
                self.clock_irq_counter()
            }
        }
    }
    fn irq_pending(&self) -> bool {
        self.irq_pending
    }
    fn reset(&mut self) {
        self.irq_enabled = false;
        self.irq_pending = false;
        self.irq_prescaler = 341
    }
    fn battery_ram(&self) -> Option<&[u8]> {
        Some(&self.prg_ram)
    }
    fn load_battery_ram(&mut self, d: &[u8]) {
        let c = d.len().min(self.prg_ram.len());
        self.prg_ram[..c].copy_from_slice(&d[..c])
    }
    fn snapshot(&self) -> MapperSnapshot {
        MapperSnapshot::Vrc(VrcSnapshot {
            mapper_id: self.mapper_id,
            submapper: self.submapper,
            prg_banks: self.prg_banks,
            chr_banks: self.chr_banks,
            mirroring: self.mirroring,
            swap_mode: self.swap_mode,
            wram_enabled: self.wram_enabled,
            vrc2_latch: self.vrc2_latch,
            irq_latch: self.irq_latch,
            irq_counter: self.irq_counter,
            irq_prescaler: self.irq_prescaler,
            irq_mode_cycle: self.irq_mode_cycle,
            irq_enable_after_ack: self.irq_enable_after_ack,
            irq_enabled: self.irq_enabled,
            irq_pending: self.irq_pending,
            prg_ram: self.prg_ram.clone(),
            chr: self.chr.clone(),
        })
    }
    fn restore_snapshot(&mut self, s: &MapperSnapshot) -> bool {
        let MapperSnapshot::Vrc(s) = s else {
            return false;
        };
        if s.mapper_id != self.mapper_id
            || s.submapper != self.submapper
            || s.prg_ram.len() != self.prg_ram.len()
            || s.chr.len() != self.chr.len()
        {
            return false;
        }
        self.prg_banks = s.prg_banks;
        self.chr_banks = s.chr_banks;
        self.mirroring = s.mirroring;
        self.swap_mode = s.swap_mode;
        self.wram_enabled = s.wram_enabled;
        self.vrc2_latch = s.vrc2_latch;
        self.irq_latch = s.irq_latch;
        self.irq_counter = s.irq_counter;
        self.irq_prescaler = s.irq_prescaler;
        self.irq_mode_cycle = s.irq_mode_cycle;
        self.irq_enable_after_ack = s.irq_enable_after_ack;
        self.irq_enabled = s.irq_enabled;
        self.irq_pending = s.irq_pending;
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
    fn vrc4_decodes_banks_and_cycle_irq() {
        let mut p = vec![0; 8 * 0x2000];
        for b in 0..8 {
            p[b * 0x2000] = b as u8
        }
        let mut m = Vrc::new(23, 1, p, vec![], 0x2000, None).unwrap();
        m.cpu_write(0x8000, 3);
        assert_eq!(m.cpu_read(0x8000), Some(3));
        m.cpu_write(0xf000, 0x0e);
        m.cpu_write(0xf001, 0x0f);
        m.cpu_write(0xf002, 0x06);
        m.clock_cpu();
        assert!(!m.irq_pending());
        m.clock_cpu();
        assert!(m.irq_pending());
    }
}

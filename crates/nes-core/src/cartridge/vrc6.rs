use super::{
    CartridgeError, Mirroring,
    mapper::{Mapper, MapperSnapshot, bank_offset, load_trainer},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
struct Pulse {
    control: u8,
    period: u16,
    counter: u16,
    step: u8,
    enabled: bool,
}
impl Pulse {
    fn write(&mut self, r: usize, v: u8) {
        match r {
            0 => self.control = v,
            1 => self.period = (self.period & 0x0f00) | u16::from(v),
            _ => {
                self.period = (self.period & 0x00ff) | (u16::from(v & 0x0f) << 8);
                self.enabled = v & 0x80 != 0
            }
        }
    }
    fn clock(&mut self) {
        if !self.enabled {
            return;
        }
        if self.counter == 0 {
            self.counter = self.period;
            self.step = (self.step + 1) & 15
        } else {
            self.counter -= 1
        }
    }
    fn output(&self) -> f32 {
        if !self.enabled {
            return 0.0;
        }
        let volume = f32::from(self.control & 15) / 15.0;
        let duty = (self.control >> 4) & 7;
        if self.control & 0x80 != 0 || self.step <= duty {
            volume * 0.06
        } else {
            0.0
        }
    }
}
#[derive(Clone, Copy, Default, Serialize, Deserialize)]
struct Saw {
    rate: u8,
    period: u16,
    counter: u16,
    step: u8,
    accumulator: u8,
    enabled: bool,
}
impl Saw {
    fn write(&mut self, r: usize, v: u8) {
        match r {
            0 => self.rate = v & 0x3f,
            1 => self.period = (self.period & 0x0f00) | u16::from(v),
            _ => {
                self.period = (self.period & 0x00ff) | (u16::from(v & 15) << 8);
                self.enabled = v & 0x80 != 0;
                if !self.enabled {
                    self.step = 0;
                    self.accumulator = 0
                }
            }
        }
    }
    fn clock(&mut self) {
        if !self.enabled {
            return;
        }
        if self.counter == 0 {
            self.counter = self.period;
            if self.step & 1 == 0 {
                self.accumulator = self.accumulator.wrapping_add(self.rate)
            }
            self.step += 1;
            if self.step >= 14 {
                self.step = 0;
                self.accumulator = 0
            }
        } else {
            self.counter -= 1
        }
    }
    fn output(&self) -> f32 {
        if self.enabled {
            f32::from(self.accumulator >> 3) / 31.0 * 0.08
        } else {
            0.0
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Vrc6Snapshot {
    mapper_id: u16,
    prg_16: u8,
    prg_8: u8,
    chr_banks: [u8; 8],
    banking: u8,
    irq_latch: u8,
    irq_counter: u8,
    irq_prescaler: i16,
    irq_mode_cycle: bool,
    irq_after_ack: bool,
    irq_enabled: bool,
    irq_pending: bool,
    pulses: [Pulse; 2],
    saw: Saw,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
}
pub struct Vrc6 {
    mapper_id: u16,
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    chr_is_ram: bool,
    prg_16: u8,
    prg_8: u8,
    chr_banks: [u8; 8],
    banking: u8,
    irq_latch: u8,
    irq_counter: u8,
    irq_prescaler: i16,
    irq_mode_cycle: bool,
    irq_after_ack: bool,
    irq_enabled: bool,
    irq_pending: bool,
    pulses: [Pulse; 2],
    saw: Saw,
}
impl Vrc6 {
    pub fn new(
        mapper_id: u16,
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
        Ok(Self {
            mapper_id,
            prg_rom,
            prg_ram,
            chr: if chr_is_ram { vec![0; 0x2000] } else { chr },
            chr_is_ram,
            prg_16: 0,
            prg_8: 0,
            chr_banks: [0; 8],
            banking: 0x20,
            irq_latch: 0,
            irq_counter: 0,
            irq_prescaler: 341,
            irq_mode_cycle: false,
            irq_after_ack: false,
            irq_enabled: false,
            irq_pending: false,
            pulses: [Pulse::default(); 2],
            saw: Saw::default(),
        })
    }
    fn reg(&self, a: u16) -> usize {
        let i = usize::from(a & 3);
        if self.mapper_id == 26 {
            ((i & 1) << 1) | ((i & 2) >> 1)
        } else {
            i
        }
    }
    fn clock_irq(&mut self) {
        if self.irq_counter == 0xff {
            self.irq_counter = self.irq_latch;
            self.irq_pending = true
        } else {
            self.irq_counter = self.irq_counter.wrapping_add(1)
        }
    }
}
impl Mapper for Vrc6 {
    fn cpu_read(&mut self, a: u16) -> Option<u8> {
        self.cpu_peek(a)
    }
    fn cpu_peek(&self, a: u16) -> Option<u8> {
        match a {
            0x6000..=0x7fff if self.banking & 0x80 != 0 => {
                Some(self.prg_ram[usize::from(a - 0x6000) % self.prg_ram.len()])
            }
            0x8000..=0xbfff => Some(
                self.prg_rom[bank_offset(
                    usize::from(self.prg_16),
                    0x4000,
                    usize::from(a - 0x8000),
                    self.prg_rom.len(),
                )],
            ),
            0xc000..=0xdfff => Some(
                self.prg_rom[bank_offset(
                    usize::from(self.prg_8),
                    0x2000,
                    usize::from(a & 0x1fff),
                    self.prg_rom.len(),
                )],
            ),
            0xe000..=0xffff => {
                Some(self.prg_rom[self.prg_rom.len() - 0x2000 + usize::from(a - 0xe000)])
            }
            _ => None,
        }
    }
    fn cpu_write(&mut self, a: u16, v: u8) -> bool {
        let r = self.reg(a);
        match a {
            0x6000..=0x7fff => {
                if self.banking & 0x80 != 0 {
                    let o = usize::from(a - 0x6000) % self.prg_ram.len();
                    self.prg_ram[o] = v
                }
                true
            }
            0x8000..=0x8fff => {
                self.prg_16 = v & 0x0f;
                true
            }
            0x9000..=0x9fff if r < 3 => {
                self.pulses[0].write(r, v);
                true
            }
            0xa000..=0xafff if r < 3 => {
                self.pulses[1].write(r, v);
                true
            }
            0xb000..=0xbfff if r < 3 => {
                self.saw.write(r, v);
                true
            }
            0xb000..=0xbfff if r == 3 => {
                self.banking = v;
                true
            }
            0xc000..=0xcfff => {
                self.prg_8 = v & 0x1f;
                true
            }
            0xd000..=0xefff => {
                let group = usize::from((a >> 12) - 0x0d);
                self.chr_banks[group * 4 + r] = v;
                true
            }
            0xf000..=0xffff => {
                match r {
                    0 => self.irq_latch = v,
                    1 => {
                        self.irq_after_ack = v & 1 != 0;
                        self.irq_enabled = v & 2 != 0;
                        self.irq_mode_cycle = v & 4 != 0;
                        self.irq_pending = false;
                        self.irq_prescaler = 341;
                        if self.irq_enabled {
                            self.irq_counter = self.irq_latch
                        }
                    }
                    2 => {
                        self.irq_pending = false;
                        self.irq_enabled = self.irq_after_ack
                    }
                    _ => {}
                }
                true
            }
            _ => false,
        }
    }
    fn ppu_read(&mut self, a: u16) -> Option<u8> {
        (a <= 0x1fff).then(|| {
            let s = usize::from(a / 0x0400);
            self.chr[bank_offset(
                usize::from(self.chr_banks[s]),
                0x0400,
                usize::from(a & 0x03ff),
                self.chr.len(),
            )]
        })
    }
    fn ppu_write(&mut self, a: u16, v: u8) -> bool {
        if a > 0x1fff {
            return false;
        }
        if self.chr_is_ram {
            let s = usize::from(a / 0x0400);
            let o = bank_offset(
                usize::from(self.chr_banks[s]),
                0x0400,
                usize::from(a & 0x03ff),
                self.chr.len(),
            );
            self.chr[o] = v
        }
        true
    }
    fn mirroring(&self) -> Option<Mirroring> {
        Some(match self.banking & 0x0c {
            0 => Mirroring::Vertical,
            4 => Mirroring::Horizontal,
            8 => Mirroring::SingleScreenLower,
            _ => Mirroring::SingleScreenUpper,
        })
    }
    fn clock_cpu(&mut self) {
        self.pulses[0].clock();
        self.pulses[1].clock();
        self.saw.clock();
        if self.irq_enabled {
            if self.irq_mode_cycle {
                self.clock_irq()
            } else {
                self.irq_prescaler -= 3;
                if self.irq_prescaler <= 0 {
                    self.irq_prescaler += 341;
                    self.clock_irq()
                }
            }
        }
    }
    fn irq_pending(&self) -> bool {
        self.irq_pending
    }
    fn expansion_audio(&self) -> f32 {
        self.pulses[0].output() + self.pulses[1].output() + self.saw.output()
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
        MapperSnapshot::Vrc6(Vrc6Snapshot {
            mapper_id: self.mapper_id,
            prg_16: self.prg_16,
            prg_8: self.prg_8,
            chr_banks: self.chr_banks,
            banking: self.banking,
            irq_latch: self.irq_latch,
            irq_counter: self.irq_counter,
            irq_prescaler: self.irq_prescaler,
            irq_mode_cycle: self.irq_mode_cycle,
            irq_after_ack: self.irq_after_ack,
            irq_enabled: self.irq_enabled,
            irq_pending: self.irq_pending,
            pulses: self.pulses,
            saw: self.saw,
            prg_ram: self.prg_ram.clone(),
            chr: self.chr.clone(),
        })
    }
    fn restore_snapshot(&mut self, s: &MapperSnapshot) -> bool {
        let MapperSnapshot::Vrc6(s) = s else {
            return false;
        };
        if s.mapper_id != self.mapper_id
            || s.prg_ram.len() != self.prg_ram.len()
            || s.chr.len() != self.chr.len()
        {
            return false;
        }
        self.prg_16 = s.prg_16;
        self.prg_8 = s.prg_8;
        self.chr_banks = s.chr_banks;
        self.banking = s.banking;
        self.irq_latch = s.irq_latch;
        self.irq_counter = s.irq_counter;
        self.irq_prescaler = s.irq_prescaler;
        self.irq_mode_cycle = s.irq_mode_cycle;
        self.irq_after_ack = s.irq_after_ack;
        self.irq_enabled = s.irq_enabled;
        self.irq_pending = s.irq_pending;
        self.pulses = s.pulses;
        self.saw = s.saw;
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
    fn banks_and_generates_audio() {
        let mut p = vec![0; 4 * 0x4000];
        for b in 0..4 {
            p[b * 0x4000] = b as u8
        }
        let mut m = Vrc6::new(24, p, vec![], 0x2000, None).unwrap();
        m.cpu_write(0x8000, 2);
        assert_eq!(m.cpu_read(0x8000), Some(2));
        m.cpu_write(0x9000, 0x8f);
        m.cpu_write(0x9001, 1);
        m.cpu_write(0x9002, 0x80);
        for _ in 0..4 {
            m.clock_cpu()
        }
        assert!(m.expansion_audio() > 0.0);
    }
}

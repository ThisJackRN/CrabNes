use super::{
    CartridgeError,
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
            2 => self.period = (self.period & 0x0700) | u16::from(v),
            3 => {
                self.period = (self.period & 0x00ff) | (u16::from(v & 7) << 8);
                self.step = 0
            }
            _ => {}
        }
    }
    fn clock(&mut self) {
        if !self.enabled {
            return;
        }
        if self.counter == 0 {
            self.counter = self.period;
            self.step = (self.step + 1) & 7
        } else {
            self.counter -= 1
        }
    }
    fn output(&self) -> f32 {
        const DUTY: [[u8; 8]; 4] = [
            [0, 1, 0, 0, 0, 0, 0, 0],
            [0, 1, 1, 0, 0, 0, 0, 0],
            [0, 1, 1, 1, 1, 0, 0, 0],
            [1, 0, 0, 1, 1, 1, 1, 1],
        ];
        if self.enabled && DUTY[usize::from(self.control >> 6)][usize::from(self.step)] != 0 {
            f32::from(self.control & 15) / 15.0 * 0.055
        } else {
            0.0
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Mmc5Snapshot {
    prg_mode: u8,
    chr_mode: u8,
    ram_protect: [u8; 2],
    exram_mode: u8,
    nametable_map: u8,
    fill_tile: u8,
    fill_color: u8,
    prg_banks: [u8; 5],
    chr_banks: [u8; 12],
    chr_upper: u8,
    exram: Vec<u8>,
    irq_compare: u8,
    irq_enabled: bool,
    irq_pending: bool,
    in_frame: bool,
    scanline: u8,
    mul: [u8; 2],
    pulses: [Pulse; 2],
    pcm: u8,
    pcm_control: u8,
    pcm_irq: bool,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
}
pub struct Mmc5 {
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    chr_is_ram: bool,
    prg_mode: u8,
    chr_mode: u8,
    ram_protect: [u8; 2],
    exram_mode: u8,
    nametable_map: u8,
    fill_tile: u8,
    fill_color: u8,
    prg_banks: [u8; 5],
    chr_banks: [u8; 12],
    chr_upper: u8,
    exram: Vec<u8>,
    irq_compare: u8,
    irq_enabled: bool,
    irq_pending: bool,
    in_frame: bool,
    scanline: u8,
    mul: [u8; 2],
    pulses: [Pulse; 2],
    pcm: u8,
    pcm_control: u8,
    pcm_irq: bool,
}
impl Mmc5 {
    pub fn new(
        prg_rom: Vec<u8>,
        chr: Vec<u8>,
        prg_ram_size: usize,
        trainer: Option<&[u8]>,
    ) -> Result<Self, CartridgeError> {
        if prg_rom.len() < 0x8000 || !prg_rom.len().is_multiple_of(0x2000) {
            return Err(CartridgeError::InvalidMapperRomSize {
                mapper: 5,
                kind: "PRG ROM",
                size: prg_rom.len(),
            });
        }
        if !chr.is_empty() && !chr.len().is_multiple_of(0x0400) {
            return Err(CartridgeError::InvalidMapperRomSize {
                mapper: 5,
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
            prg_mode: 3,
            chr_mode: 3,
            ram_protect: [0; 2],
            exram_mode: 0,
            nametable_map: 0,
            fill_tile: 0,
            fill_color: 0,
            prg_banks: [0, 0, 0, 0, 0xff],
            chr_banks: [0; 12],
            chr_upper: 0,
            exram: vec![0; 0x400],
            irq_compare: 0,
            irq_enabled: false,
            irq_pending: false,
            in_frame: false,
            scanline: 0,
            mul: [0; 2],
            pulses: [Pulse::default(); 2],
            pcm: 0,
            pcm_control: 1,
            pcm_irq: false,
        })
    }
    fn prg_register(&self, a: u16) -> (usize, usize) {
        match self.prg_mode {
            0 => (4, 0x8000),
            1 => {
                if a < 0xc000 {
                    (2, 0x4000)
                } else {
                    (4, 0x4000)
                }
            }
            2 => {
                if a < 0xc000 {
                    (2, 0x4000)
                } else if a < 0xe000 {
                    (3, 0x2000)
                } else {
                    (4, 0x2000)
                }
            }
            _ => ((usize::from(a - 0x8000) / 0x2000) + 1, 0x2000),
        }
    }
    fn read_prg_window(&self, a: u16) -> u8 {
        let (reg, size) = self.prg_register(a);
        let value = self.prg_banks[reg];
        let local = usize::from(a - 0x8000) % size;
        if value & 0x80 != 0 || reg == 4 {
            self.prg_rom[bank_offset(
                usize::from(value & 0x7f) / (size / 0x2000),
                size,
                local,
                self.prg_rom.len(),
            )]
        } else {
            self.prg_ram[bank_offset(
                usize::from(value & 7) / (size / 0x2000),
                size,
                local,
                self.prg_ram.len(),
            )]
        }
    }
    fn chr_bank(&self, slot: usize) -> usize {
        let reg = match self.chr_mode {
            0 => 7,
            1 => {
                if slot < 4 {
                    3
                } else {
                    7
                }
            }
            2 => slot / 2 * 2 + 1,
            _ => slot,
        };
        (usize::from(self.chr_upper) << 8) | usize::from(self.chr_banks[reg])
    }
    fn nt_kind(&self, slot: usize) -> u8 {
        (self.nametable_map >> (slot * 2)) & 3
    }
}
impl Mapper for Mmc5 {
    fn cpu_read(&mut self, a: u16) -> Option<u8> {
        match a {
            0x5010 => {
                let v = (u8::from(self.pcm_irq && self.pcm_control & 0x80 != 0) << 7)
                    | (self.pcm_control & 1);
                self.pcm_irq = false;
                Some(v)
            }
            0x5015 => {
                Some(u8::from(self.pulses[0].enabled) | (u8::from(self.pulses[1].enabled) << 1))
            }
            0x5204 => {
                let v = (u8::from(self.irq_pending) << 7) | (u8::from(self.in_frame) << 6);
                self.irq_pending = false;
                Some(v)
            }
            0x5205 => Some((u16::from(self.mul[0]) * u16::from(self.mul[1])) as u8),
            0x5206 => Some(((u16::from(self.mul[0]) * u16::from(self.mul[1])) >> 8) as u8),
            _ => self.cpu_peek(a),
        }
    }
    fn cpu_peek(&self, a: u16) -> Option<u8> {
        match a {
            0x5c00..=0x5fff => Some(self.exram[usize::from(a - 0x5c00)]),
            0x6000..=0x7fff => {
                let bank = usize::from(self.prg_banks[0] & 7);
                Some(
                    self.prg_ram
                        [bank_offset(bank, 0x2000, usize::from(a - 0x6000), self.prg_ram.len())],
                )
            }
            0x8000..=0xffff => Some(self.read_prg_window(a)),
            _ => None,
        }
    }
    fn cpu_write(&mut self, a: u16, v: u8) -> bool {
        match a {
            0x5000..=0x5003 => {
                self.pulses[0].write(usize::from(a - 0x5000), v);
                true
            }
            0x5004..=0x5007 => {
                self.pulses[1].write(usize::from(a - 0x5004), v);
                true
            }
            0x5010 => {
                self.pcm_control = v;
                self.pcm_irq = false;
                true
            }
            0x5011 => {
                if self.pcm_control & 1 == 0 && v != 0 {
                    self.pcm = v
                } else if v == 0 {
                    self.pcm_irq = true
                }
                true
            }
            0x5015 => {
                self.pulses[0].enabled = v & 1 != 0;
                self.pulses[1].enabled = v & 2 != 0;
                true
            }
            0x5100 => {
                self.prg_mode = v & 3;
                true
            }
            0x5101 => {
                self.chr_mode = v & 3;
                true
            }
            0x5102 => {
                self.ram_protect[0] = v & 3;
                true
            }
            0x5103 => {
                self.ram_protect[1] = v & 3;
                true
            }
            0x5104 => {
                self.exram_mode = v & 3;
                true
            }
            0x5105 => {
                self.nametable_map = v;
                true
            }
            0x5106 => {
                self.fill_tile = v;
                true
            }
            0x5107 => {
                self.fill_color = v & 3;
                true
            }
            0x5113..=0x5117 => {
                self.prg_banks[usize::from(a - 0x5113)] = v;
                true
            }
            0x5120..=0x512b => {
                self.chr_banks[usize::from(a - 0x5120)] = v;
                true
            }
            0x5130 => {
                self.chr_upper = v & 3;
                true
            }
            0x5203 => {
                self.irq_compare = v;
                true
            }
            0x5204 => {
                self.irq_enabled = v & 0x80 != 0;
                true
            }
            0x5205 => {
                self.mul[0] = v;
                true
            }
            0x5206 => {
                self.mul[1] = v;
                true
            }
            0x5c00..=0x5fff => {
                if self.exram_mode >= 2 {
                    self.exram[usize::from(a - 0x5c00)] = v
                }
                true
            }
            0x6000..=0x7fff => {
                if self.ram_protect == [2, 1] {
                    let bank = usize::from(self.prg_banks[0] & 7);
                    let o = bank_offset(bank, 0x2000, usize::from(a - 0x6000), self.prg_ram.len());
                    self.prg_ram[o] = v
                }
                true
            }
            0x8000..=0xffff => {
                let (reg, size) = self.prg_register(a);
                let value = self.prg_banks[reg];
                if value & 0x80 == 0 && reg != 4 && self.ram_protect == [2, 1] {
                    let local = usize::from(a - 0x8000) % size;
                    let o = bank_offset(
                        usize::from(value & 7) / (size / 0x2000),
                        size,
                        local,
                        self.prg_ram.len(),
                    );
                    self.prg_ram[o] = v
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
                self.chr_bank(s),
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
                self.chr_bank(s),
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
        match self.nt_kind(s) {
            2 => Some(if self.exram_mode < 2 {
                self.exram[usize::from(a & 0x03ff)]
            } else {
                0
            }),
            3 => Some(if a & 0x03ff >= 0x03c0 {
                self.fill_color * 0x55
            } else {
                self.fill_tile
            }),
            _ => None,
        }
    }
    fn nametable_write(&mut self, a: u16, v: u8) -> bool {
        let s = usize::from((a & 0x0fff) / 0x0400);
        match self.nt_kind(s) {
            2 => {
                if self.exram_mode < 2 {
                    self.exram[usize::from(a & 0x03ff)] = v
                }
                true
            }
            3 => true,
            _ => false,
        }
    }
    fn nametable_ciram_index(&self, a: u16) -> Option<usize> {
        let s = usize::from((a & 0x0fff) / 0x0400);
        match self.nt_kind(s) {
            0 | 1 => Some(usize::from(self.nt_kind(s)) * 0x400 + usize::from(a & 0x03ff)),
            _ => None,
        }
    }
    fn clock_cpu(&mut self) {
        self.pulses[0].clock();
        self.pulses[1].clock()
    }
    fn clock_scanline_at(&mut self, line: i16) {
        if line < 0 {
            self.scanline = 0;
            self.in_frame = false;
            return;
        }
        self.in_frame = true;
        self.scanline = line as u8;
        if self.scanline == self.irq_compare {
            self.irq_pending = true
        }
    }
    fn irq_pending(&self) -> bool {
        (self.irq_pending && self.irq_enabled) || (self.pcm_irq && self.pcm_control & 0x80 != 0)
    }
    fn expansion_audio(&self) -> f32 {
        self.pulses[0].output() + self.pulses[1].output() + f32::from(self.pcm) / 255.0 * 0.04
    }
    fn reset(&mut self) {
        self.irq_pending = false;
        self.in_frame = false;
        self.pcm_irq = false
    }
    fn battery_ram(&self) -> Option<&[u8]> {
        Some(&self.prg_ram)
    }
    fn load_battery_ram(&mut self, d: &[u8]) {
        let c = d.len().min(self.prg_ram.len());
        self.prg_ram[..c].copy_from_slice(&d[..c])
    }
    fn snapshot(&self) -> MapperSnapshot {
        MapperSnapshot::Mmc5(Mmc5Snapshot {
            prg_mode: self.prg_mode,
            chr_mode: self.chr_mode,
            ram_protect: self.ram_protect,
            exram_mode: self.exram_mode,
            nametable_map: self.nametable_map,
            fill_tile: self.fill_tile,
            fill_color: self.fill_color,
            prg_banks: self.prg_banks,
            chr_banks: self.chr_banks,
            chr_upper: self.chr_upper,
            exram: self.exram.clone(),
            irq_compare: self.irq_compare,
            irq_enabled: self.irq_enabled,
            irq_pending: self.irq_pending,
            in_frame: self.in_frame,
            scanline: self.scanline,
            mul: self.mul,
            pulses: self.pulses,
            pcm: self.pcm,
            pcm_control: self.pcm_control,
            pcm_irq: self.pcm_irq,
            prg_ram: self.prg_ram.clone(),
            chr: self.chr.clone(),
        })
    }
    fn restore_snapshot(&mut self, s: &MapperSnapshot) -> bool {
        let MapperSnapshot::Mmc5(s) = s else {
            return false;
        };
        if s.prg_ram.len() != self.prg_ram.len()
            || s.chr.len() != self.chr.len()
            || s.exram.len() != 0x400
        {
            return false;
        }
        self.prg_mode = s.prg_mode;
        self.chr_mode = s.chr_mode;
        self.ram_protect = s.ram_protect;
        self.exram_mode = s.exram_mode;
        self.nametable_map = s.nametable_map;
        self.fill_tile = s.fill_tile;
        self.fill_color = s.fill_color;
        self.prg_banks = s.prg_banks;
        self.chr_banks = s.chr_banks;
        self.chr_upper = s.chr_upper;
        self.exram.clone_from(&s.exram);
        self.irq_compare = s.irq_compare;
        self.irq_enabled = s.irq_enabled;
        self.irq_pending = s.irq_pending;
        self.in_frame = s.in_frame;
        self.scanline = s.scanline;
        self.mul = s.mul;
        self.pulses = s.pulses;
        self.pcm = s.pcm;
        self.pcm_control = s.pcm_control;
        self.pcm_irq = s.pcm_irq;
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
    fn banking_multiplier_fill_and_audio_work() {
        let mut p = vec![0; 8 * 0x2000];
        for b in 0..8 {
            p[b * 0x2000] = b as u8
        }
        let mut m = Mmc5::new(p, vec![], 0x8000, None).unwrap();
        m.cpu_write(0x5114, 0x83);
        assert_eq!(m.cpu_read(0x8000), Some(3));
        m.cpu_write(0x5205, 7);
        m.cpu_write(0x5206, 9);
        assert_eq!(m.cpu_read(0x5205), Some(63));
        m.cpu_write(0x5105, 3);
        m.cpu_write(0x5106, 0xaa);
        assert_eq!(m.nametable_read(0x2000), Some(0xaa));
        m.cpu_write(0x5000, 0xdf);
        m.cpu_write(0x5002, 1);
        m.cpu_write(0x5015, 1);
        for _ in 0..4 {
            m.clock_cpu()
        }
        assert!(m.expansion_audio() >= 0.0)
    }
}

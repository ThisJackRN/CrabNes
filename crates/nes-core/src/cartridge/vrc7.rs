use super::{
    CartridgeError, Mirroring,
    mapper::{Mapper, MapperSnapshot, bank_offset, load_trainer},
};
use crate::CPU_CLOCK_HZ;
use serde::{Deserialize, Serialize};
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Vrc7Snapshot {
    submapper: u8,
    prg_banks: [u8; 3],
    chr_banks: [u8; 8],
    mirroring: Mirroring,
    wram_enabled: bool,
    irq_latch: u8,
    irq_counter: u8,
    irq_prescaler: i16,
    irq_mode_cycle: bool,
    irq_after_ack: bool,
    irq_enabled: bool,
    irq_pending: bool,
    audio_reset: bool,
    audio_select: u8,
    audio_registers: Vec<u8>,
    phases: [f64; 6],
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
}
pub struct Vrc7 {
    submapper: u8,
    prg_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr: Vec<u8>,
    chr_is_ram: bool,
    prg_banks: [u8; 3],
    chr_banks: [u8; 8],
    mirroring: Mirroring,
    wram_enabled: bool,
    irq_latch: u8,
    irq_counter: u8,
    irq_prescaler: i16,
    irq_mode_cycle: bool,
    irq_after_ack: bool,
    irq_enabled: bool,
    irq_pending: bool,
    audio_reset: bool,
    audio_select: u8,
    audio_registers: Vec<u8>,
    phases: [f64; 6],
}
impl Vrc7 {
    pub fn new(
        submapper: u8,
        prg_rom: Vec<u8>,
        chr: Vec<u8>,
        prg_ram_size: usize,
        trainer: Option<&[u8]>,
    ) -> Result<Self, CartridgeError> {
        if prg_rom.len() < 0x8000 || !prg_rom.len().is_multiple_of(0x2000) {
            return Err(CartridgeError::InvalidMapperRomSize {
                mapper: 85,
                kind: "PRG ROM",
                size: prg_rom.len(),
            });
        }
        if !chr.is_empty() && !chr.len().is_multiple_of(0x0400) {
            return Err(CartridgeError::InvalidMapperRomSize {
                mapper: 85,
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
            prg_banks: [0, 1, 2],
            chr_banks: [0; 8],
            mirroring: Mirroring::Vertical,
            wram_enabled: false,
            irq_latch: 0,
            irq_counter: 0,
            irq_prescaler: 341,
            irq_mode_cycle: false,
            irq_after_ack: false,
            irq_enabled: false,
            irq_pending: false,
            audio_reset: false,
            audio_select: 0,
            audio_registers: vec![0; 0x40],
            phases: [0.0; 6],
        })
    }
    fn secondary(&self, a: u16) -> bool {
        if self.submapper == 1 {
            a & 8 != 0
        } else {
            a & 0x10 != 0
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
    fn write_audio(&mut self, v: u8) {
        let r = usize::from(self.audio_select);
        if r < self.audio_registers.len() {
            self.audio_registers[r] = v
        }
    }
    fn audio_level(&self) -> f32 {
        if self.audio_reset {
            return 0.0;
        }
        let mut mix = 0.0;
        for ch in 0..6 {
            let high = self.audio_registers[0x20 + ch];
            if high & 0x10 == 0 {
                continue;
            }
            let instrument = self.audio_registers[0x30 + ch] >> 4;
            let volume = self.audio_registers[0x30 + ch] & 15;
            let depth = f64::from((instrument % 8) + 1) * 0.08;
            let amplitude = 10_f64.powf(-f64::from(volume) * 3.0 / 20.0);
            mix += (self.phases[ch].sin() + (self.phases[ch] * 2.0).sin() * depth).sin() * amplitude
        }
        mix as f32 / 6.0 * 0.10
    }
}
impl Mapper for Vrc7 {
    fn cpu_read(&mut self, a: u16) -> Option<u8> {
        self.cpu_peek(a)
    }
    fn cpu_peek(&self, a: u16) -> Option<u8> {
        match a {
            0x6000..=0x7fff if self.wram_enabled => {
                Some(self.prg_ram[usize::from(a - 0x6000) % self.prg_ram.len()])
            }
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
            0x6000..=0x7fff => {
                if self.wram_enabled {
                    let o = usize::from(a - 0x6000) % self.prg_ram.len();
                    self.prg_ram[o] = v
                }
                true
            }
            0x8000..=0x8fff => {
                let s = usize::from(self.secondary(a));
                self.prg_banks[s] = v & 0x3f;
                true
            }
            0x9000..=0x9fff if a & 0x30 == 0x30 => {
                self.write_audio(v);
                true
            }
            0x9000..=0x9fff if a & 0x30 == 0x10 => {
                self.audio_select = v;
                true
            }
            0x9000..=0x9fff => {
                self.prg_banks[2] = v & 0x3f;
                true
            }
            0xa000..=0xdfff => {
                let group = usize::from((a >> 12) - 0x0a);
                let s = group * 2 + usize::from(self.secondary(a));
                self.chr_banks[s] = v;
                true
            }
            0xe000..=0xefff if self.secondary(a) => {
                self.irq_latch = v;
                true
            }
            0xe000..=0xefff => {
                self.mirroring = match v & 3 {
                    0 => Mirroring::Vertical,
                    1 => Mirroring::Horizontal,
                    2 => Mirroring::SingleScreenLower,
                    _ => Mirroring::SingleScreenUpper,
                };
                self.audio_reset = v & 0x40 != 0;
                self.wram_enabled = v & 0x80 != 0;
                if self.audio_reset {
                    self.audio_registers.fill(0);
                    self.phases = [0.0; 6]
                }
                true
            }
            0xf000..=0xffff if self.secondary(a) => {
                self.irq_pending = false;
                self.irq_enabled = self.irq_after_ack;
                true
            }
            0xf000..=0xffff => {
                self.irq_after_ack = v & 1 != 0;
                self.irq_enabled = v & 2 != 0;
                self.irq_mode_cycle = v & 4 != 0;
                self.irq_pending = false;
                self.irq_prescaler = 341;
                if self.irq_enabled {
                    self.irq_counter = self.irq_latch
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
        Some(self.mirroring)
    }
    fn clock_cpu(&mut self) {
        for ch in 0..6 {
            let low = u16::from(self.audio_registers[0x10 + ch]);
            let high = self.audio_registers[0x20 + ch];
            if high & 0x10 != 0 {
                let freq = low | (u16::from(high & 1) << 8);
                let octave = (high >> 1) & 7;
                let hz = 49716.0 * f64::from(freq) / 2_f64.powi(19 - i32::from(octave));
                self.phases[ch] = (self.phases[ch]
                    + std::f64::consts::TAU * hz / f64::from(CPU_CLOCK_HZ))
                    % std::f64::consts::TAU
            }
        }
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
        self.audio_level()
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
        MapperSnapshot::Vrc7(Vrc7Snapshot {
            submapper: self.submapper,
            prg_banks: self.prg_banks,
            chr_banks: self.chr_banks,
            mirroring: self.mirroring,
            wram_enabled: self.wram_enabled,
            irq_latch: self.irq_latch,
            irq_counter: self.irq_counter,
            irq_prescaler: self.irq_prescaler,
            irq_mode_cycle: self.irq_mode_cycle,
            irq_after_ack: self.irq_after_ack,
            irq_enabled: self.irq_enabled,
            irq_pending: self.irq_pending,
            audio_reset: self.audio_reset,
            audio_select: self.audio_select,
            audio_registers: self.audio_registers.clone(),
            phases: self.phases,
            prg_ram: self.prg_ram.clone(),
            chr: self.chr.clone(),
        })
    }
    fn restore_snapshot(&mut self, s: &MapperSnapshot) -> bool {
        let MapperSnapshot::Vrc7(s) = s else {
            return false;
        };
        if s.submapper != self.submapper
            || s.audio_registers.len() != 0x40
            || s.prg_ram.len() != self.prg_ram.len()
            || s.chr.len() != self.chr.len()
        {
            return false;
        }
        self.prg_banks = s.prg_banks;
        self.chr_banks = s.chr_banks;
        self.mirroring = s.mirroring;
        self.wram_enabled = s.wram_enabled;
        self.irq_latch = s.irq_latch;
        self.irq_counter = s.irq_counter;
        self.irq_prescaler = s.irq_prescaler;
        self.irq_mode_cycle = s.irq_mode_cycle;
        self.irq_after_ack = s.irq_after_ack;
        self.irq_enabled = s.irq_enabled;
        self.irq_pending = s.irq_pending;
        self.audio_reset = s.audio_reset;
        self.audio_select = s.audio_select;
        self.audio_registers.clone_from(&s.audio_registers);
        self.phases = s.phases;
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
    fn banks_and_drives_fm_output() {
        for submapper in [1, 2] {
            let mut p = vec![0; 8 * 0x2000];
            for b in 0..8 {
                p[b * 0x2000] = b as u8
            }
            let mut m = Vrc7::new(submapper, p, vec![], 0x2000, None).unwrap();
            m.cpu_write(0x8000, 3);
            assert_eq!(m.cpu_read(0x8000), Some(3));
            m.cpu_write(0x9010, 0x10);
            m.cpu_write(0x9030, 0xff);
            m.cpu_write(0x9010, 0x20);
            m.cpu_write(0x9030, 0x11);
            m.cpu_write(0x9010, 0x30);
            m.cpu_write(0x9030, 0x10);
            for _ in 0..100 {
                m.clock_cpu()
            }
            assert_ne!(m.expansion_audio(), 0.0)
        }
    }
}

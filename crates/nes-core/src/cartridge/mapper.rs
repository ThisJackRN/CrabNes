use serde::{Deserialize, Serialize};

use super::{
    Mirroring, axrom::AxromSnapshot, cnrom::CnromSnapshot, fme7::Fme7Snapshot,
    mapper99::Mapper99Snapshot, mmc1::Mmc1Snapshot, mmc2::Mmc2Snapshot, mmc3::Mmc3Snapshot,
    mmc5::Mmc5Snapshot, n163::N163Snapshot, nrom::NromSnapshot, uxrom::UxromSnapshot,
    vrc::VrcSnapshot, vrc6::Vrc6Snapshot, vrc7::Vrc7Snapshot,
};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) enum MapperSnapshot {
    Nrom(NromSnapshot),
    Uxrom(UxromSnapshot),
    Cnrom(CnromSnapshot),
    Mmc1(Mmc1Snapshot),
    Axrom(AxromSnapshot),
    Mmc3(Mmc3Snapshot),
    Mmc2(Mmc2Snapshot),
    Fme7(Fme7Snapshot),
    Vrc(VrcSnapshot),
    Vrc6(Vrc6Snapshot),
    N163(N163Snapshot),
    Mmc5(Mmc5Snapshot),
    Vrc7(Vrc7Snapshot),
    Mapper99(Mapper99Snapshot),
}

pub trait Mapper {
    fn cpu_read(&mut self, address: u16) -> Option<u8>;
    fn cpu_peek(&self, address: u16) -> Option<u8>;
    fn cpu_write(&mut self, address: u16, value: u8) -> bool;
    fn ppu_read(&mut self, address: u16) -> Option<u8>;
    fn ppu_write(&mut self, address: u16, value: u8) -> bool;
    fn nametable_read(&mut self, _address: u16) -> Option<u8> {
        None
    }
    fn nametable_write(&mut self, _address: u16, _value: u8) -> bool {
        false
    }
    fn nametable_ciram_index(&self, _address: u16) -> Option<usize> {
        None
    }
    fn clock_cpu(&mut self) {}
    fn clock_scanline(&mut self) {}
    fn clock_scanline_at(&mut self, _scanline: i16) {
        self.clock_scanline();
    }
    fn irq_pending(&self) -> bool {
        false
    }
    fn expansion_audio(&self) -> f32 {
        0.0
    }
    fn reset(&mut self) {}
    fn mirroring(&self) -> Option<Mirroring> {
        None
    }
    fn battery_ram(&self) -> Option<&[u8]> {
        None
    }
    fn load_battery_ram(&mut self, _data: &[u8]) {}
    fn snapshot(&self) -> MapperSnapshot;
    fn restore_snapshot(&mut self, snapshot: &MapperSnapshot) -> bool;
    fn prg_rom(&self) -> &[u8];
    fn chr(&self) -> &[u8];
    fn chr_is_writable(&self) -> bool {
        false
    }
    fn debug_write_chr(&mut self, _offset: usize, _value: u8) -> bool {
        false
    }
}

pub(super) fn bank_offset(bank: usize, bank_size: usize, address: usize, len: usize) -> usize {
    let bank_count = (len / bank_size).max(1);
    (bank % bank_count) * bank_size + address % bank_size
}

pub(super) fn load_trainer(ram: &mut [u8], trainer: Option<&[u8]>) {
    let Some(trainer) = trainer else { return };
    // Trainers are wired to CPU $7000-$71FF. PRG RAM begins at $6000.
    if ram.len() <= 0x1000 {
        return;
    }
    let count = trainer.len().min(512).min(ram.len() - 0x1000);
    ram[0x1000..0x1000 + count].copy_from_slice(&trainer[..count]);
}

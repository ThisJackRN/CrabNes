use serde::{Deserialize, Serialize};

use super::nrom::NromSnapshot;

#[derive(Clone, Serialize, Deserialize)]
pub(crate) enum MapperSnapshot {
    Nrom(NromSnapshot),
}

pub trait Mapper {
    fn cpu_read(&mut self, address: u16) -> Option<u8>;
    fn cpu_peek(&self, address: u16) -> Option<u8>;
    fn cpu_write(&mut self, address: u16, value: u8) -> bool;
    fn ppu_read(&mut self, address: u16) -> Option<u8>;
    fn ppu_write(&mut self, address: u16, value: u8) -> bool;
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

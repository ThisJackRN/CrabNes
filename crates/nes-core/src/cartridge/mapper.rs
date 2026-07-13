pub trait Mapper {
    fn cpu_read(&mut self, address: u16) -> Option<u8>;
    fn cpu_write(&mut self, address: u16, value: u8) -> bool;
    fn ppu_read(&mut self, address: u16) -> Option<u8>;
    fn ppu_write(&mut self, address: u16, value: u8) -> bool;
    fn battery_ram(&self) -> Option<&[u8]> {
        None
    }
    fn load_battery_ram(&mut self, _data: &[u8]) {}
}

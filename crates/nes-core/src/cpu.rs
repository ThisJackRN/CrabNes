use std::{error::Error, fmt};

use crate::bus::Bus;

const C: u8 = 1 << 0;
const Z: u8 = 1 << 1;
const I: u8 = 1 << 2;
const D: u8 = 1 << 3;
const B: u8 = 1 << 4;
const U: u8 = 1 << 5;
const V: u8 = 1 << 6;
const N: u8 = 1 << 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuState {
    pub a: u8,
    pub x: u8,
    pub y: u8,
    pub stack_pointer: u8,
    pub program_counter: u16,
    pub status: u8,
    pub instructions: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuError {
    pub opcode: u8,
    pub address: u16,
}

impl fmt::Display for CpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unsupported opcode ${:02X} at ${:04X}",
            self.opcode, self.address
        )
    }
}
impl Error for CpuError {}

pub struct Cpu {
    a: u8,
    x: u8,
    y: u8,
    sp: u8,
    pc: u16,
    status: u8,
    instructions: u64,
}

impl Default for Cpu {
    fn default() -> Self {
        Self {
            a: 0,
            x: 0,
            y: 0,
            sp: 0xfd,
            pc: 0,
            status: U | I,
            instructions: 0,
        }
    }
}

impl Cpu {
    pub fn reset(&mut self, bus: &mut Bus) {
        self.a = 0;
        self.x = 0;
        self.y = 0;
        self.sp = 0xfd;
        self.status = U | I;
        self.pc = self.read_word(bus, 0xfffc);
        self.instructions = 0;
    }

    pub fn state(&self) -> CpuState {
        CpuState {
            a: self.a,
            x: self.x,
            y: self.y,
            stack_pointer: self.sp,
            program_counter: self.pc,
            status: self.status,
            instructions: self.instructions,
        }
    }

    pub fn nmi(&mut self, bus: &mut Bus) -> u16 {
        self.interrupt(bus, 0xfffa, false);
        7
    }

    pub fn irq(&mut self, bus: &mut Bus) -> u16 {
        if self.flag(I) {
            0
        } else {
            self.interrupt(bus, 0xfffe, false);
            7
        }
    }

    pub fn step(&mut self, bus: &mut Bus) -> Result<u16, CpuError> {
        let opcode_address = self.pc;
        let opcode = self.fetch(bus);
        self.instructions = self.instructions.wrapping_add(1);
        let cycles = match opcode {
            // ADC
            0x69 => {
                let v = self.fetch(bus);
                self.adc(v);
                2
            }
            0x65 => {
                let a = self.zp(bus);
                let v = bus.read(a);
                self.adc(v);
                3
            }
            0x75 => {
                let a = self.zpx(bus);
                let v = bus.read(a);
                self.adc(v);
                4
            }
            0x6d => {
                let a = self.abs(bus);
                let v = bus.read(a);
                self.adc(v);
                4
            }
            0x7d => {
                let (a, p) = self.absx(bus);
                let v = bus.read(a);
                self.adc(v);
                4 + p as u16
            }
            0x79 => {
                let (a, p) = self.absy(bus);
                let v = bus.read(a);
                self.adc(v);
                4 + p as u16
            }
            0x61 => {
                let a = self.indx(bus);
                let v = bus.read(a);
                self.adc(v);
                6
            }
            0x71 => {
                let (a, p) = self.indy(bus);
                let v = bus.read(a);
                self.adc(v);
                5 + p as u16
            }

            // AND
            0x29 => {
                let v = self.fetch(bus);
                self.a &= v;
                self.set_zn(self.a);
                2
            }
            0x25 => {
                let a = self.zp(bus);
                self.a &= bus.read(a);
                self.set_zn(self.a);
                3
            }
            0x35 => {
                let a = self.zpx(bus);
                self.a &= bus.read(a);
                self.set_zn(self.a);
                4
            }
            0x2d => {
                let a = self.abs(bus);
                self.a &= bus.read(a);
                self.set_zn(self.a);
                4
            }
            0x3d => {
                let (a, p) = self.absx(bus);
                self.a &= bus.read(a);
                self.set_zn(self.a);
                4 + p as u16
            }
            0x39 => {
                let (a, p) = self.absy(bus);
                self.a &= bus.read(a);
                self.set_zn(self.a);
                4 + p as u16
            }
            0x21 => {
                let a = self.indx(bus);
                self.a &= bus.read(a);
                self.set_zn(self.a);
                6
            }
            0x31 => {
                let (a, p) = self.indy(bus);
                self.a &= bus.read(a);
                self.set_zn(self.a);
                5 + p as u16
            }

            // ASL
            0x0a => {
                self.a = self.asl(self.a);
                2
            }
            0x06 => {
                let a = self.zp(bus);
                let v = bus.read(a);
                let r = self.asl(v);
                bus.write(a, r);
                5
            }
            0x16 => {
                let a = self.zpx(bus);
                let v = bus.read(a);
                let r = self.asl(v);
                bus.write(a, r);
                6
            }
            0x0e => {
                let a = self.abs(bus);
                let v = bus.read(a);
                let r = self.asl(v);
                bus.write(a, r);
                6
            }
            0x1e => {
                let (a, _) = self.absx(bus);
                let v = bus.read(a);
                let r = self.asl(v);
                bus.write(a, r);
                7
            }

            // Branches
            0x90 => self.branch(bus, !self.flag(C)),
            0xb0 => self.branch(bus, self.flag(C)),
            0xf0 => self.branch(bus, self.flag(Z)),
            0x30 => self.branch(bus, self.flag(N)),
            0xd0 => self.branch(bus, !self.flag(Z)),
            0x10 => self.branch(bus, !self.flag(N)),
            0x50 => self.branch(bus, !self.flag(V)),
            0x70 => self.branch(bus, self.flag(V)),

            // BIT
            0x24 => {
                let a = self.zp(bus);
                let v = bus.read(a);
                self.bit(v);
                3
            }
            0x2c => {
                let a = self.abs(bus);
                let v = bus.read(a);
                self.bit(v);
                4
            }

            0x00 => {
                self.pc = self.pc.wrapping_add(1);
                self.interrupt(bus, 0xfffe, true);
                7
            }

            // Flags
            0x18 => {
                self.set_flag(C, false);
                2
            }
            0xd8 => {
                self.set_flag(D, false);
                2
            }
            0x58 => {
                self.set_flag(I, false);
                2
            }
            0xb8 => {
                self.set_flag(V, false);
                2
            }
            0x38 => {
                self.set_flag(C, true);
                2
            }
            0xf8 => {
                self.set_flag(D, true);
                2
            }
            0x78 => {
                self.set_flag(I, true);
                2
            }

            // CMP
            0xc9 => {
                let v = self.fetch(bus);
                self.compare(self.a, v);
                2
            }
            0xc5 => {
                let a = self.zp(bus);
                let v = bus.read(a);
                self.compare(self.a, v);
                3
            }
            0xd5 => {
                let a = self.zpx(bus);
                let v = bus.read(a);
                self.compare(self.a, v);
                4
            }
            0xcd => {
                let a = self.abs(bus);
                let v = bus.read(a);
                self.compare(self.a, v);
                4
            }
            0xdd => {
                let (a, p) = self.absx(bus);
                let v = bus.read(a);
                self.compare(self.a, v);
                4 + p as u16
            }
            0xd9 => {
                let (a, p) = self.absy(bus);
                let v = bus.read(a);
                self.compare(self.a, v);
                4 + p as u16
            }
            0xc1 => {
                let a = self.indx(bus);
                let v = bus.read(a);
                self.compare(self.a, v);
                6
            }
            0xd1 => {
                let (a, p) = self.indy(bus);
                let v = bus.read(a);
                self.compare(self.a, v);
                5 + p as u16
            }
            // CPX/CPY
            0xe0 => {
                let v = self.fetch(bus);
                self.compare(self.x, v);
                2
            }
            0xe4 => {
                let a = self.zp(bus);
                let v = bus.read(a);
                self.compare(self.x, v);
                3
            }
            0xec => {
                let a = self.abs(bus);
                let v = bus.read(a);
                self.compare(self.x, v);
                4
            }
            0xc0 => {
                let v = self.fetch(bus);
                self.compare(self.y, v);
                2
            }
            0xc4 => {
                let a = self.zp(bus);
                let v = bus.read(a);
                self.compare(self.y, v);
                3
            }
            0xcc => {
                let a = self.abs(bus);
                let v = bus.read(a);
                self.compare(self.y, v);
                4
            }

            // DEC/DEX/DEY
            0xc6 => {
                let a = self.zp(bus);
                let v = bus.read(a).wrapping_sub(1);
                bus.write(a, v);
                self.set_zn(v);
                5
            }
            0xd6 => {
                let a = self.zpx(bus);
                let v = bus.read(a).wrapping_sub(1);
                bus.write(a, v);
                self.set_zn(v);
                6
            }
            0xce => {
                let a = self.abs(bus);
                let v = bus.read(a).wrapping_sub(1);
                bus.write(a, v);
                self.set_zn(v);
                6
            }
            0xde => {
                let (a, _) = self.absx(bus);
                let v = bus.read(a).wrapping_sub(1);
                bus.write(a, v);
                self.set_zn(v);
                7
            }
            0xca => {
                self.x = self.x.wrapping_sub(1);
                self.set_zn(self.x);
                2
            }
            0x88 => {
                self.y = self.y.wrapping_sub(1);
                self.set_zn(self.y);
                2
            }

            // EOR
            0x49 => {
                let v = self.fetch(bus);
                self.a ^= v;
                self.set_zn(self.a);
                2
            }
            0x45 => {
                let a = self.zp(bus);
                self.a ^= bus.read(a);
                self.set_zn(self.a);
                3
            }
            0x55 => {
                let a = self.zpx(bus);
                self.a ^= bus.read(a);
                self.set_zn(self.a);
                4
            }
            0x4d => {
                let a = self.abs(bus);
                self.a ^= bus.read(a);
                self.set_zn(self.a);
                4
            }
            0x5d => {
                let (a, p) = self.absx(bus);
                self.a ^= bus.read(a);
                self.set_zn(self.a);
                4 + p as u16
            }
            0x59 => {
                let (a, p) = self.absy(bus);
                self.a ^= bus.read(a);
                self.set_zn(self.a);
                4 + p as u16
            }
            0x41 => {
                let a = self.indx(bus);
                self.a ^= bus.read(a);
                self.set_zn(self.a);
                6
            }
            0x51 => {
                let (a, p) = self.indy(bus);
                self.a ^= bus.read(a);
                self.set_zn(self.a);
                5 + p as u16
            }

            // INC/INX/INY
            0xe6 => {
                let a = self.zp(bus);
                let v = bus.read(a).wrapping_add(1);
                bus.write(a, v);
                self.set_zn(v);
                5
            }
            0xf6 => {
                let a = self.zpx(bus);
                let v = bus.read(a).wrapping_add(1);
                bus.write(a, v);
                self.set_zn(v);
                6
            }
            0xee => {
                let a = self.abs(bus);
                let v = bus.read(a).wrapping_add(1);
                bus.write(a, v);
                self.set_zn(v);
                6
            }
            0xfe => {
                let (a, _) = self.absx(bus);
                let v = bus.read(a).wrapping_add(1);
                bus.write(a, v);
                self.set_zn(v);
                7
            }
            0xe8 => {
                self.x = self.x.wrapping_add(1);
                self.set_zn(self.x);
                2
            }
            0xc8 => {
                self.y = self.y.wrapping_add(1);
                self.set_zn(self.y);
                2
            }

            // JMP/JSR
            0x4c => {
                self.pc = self.abs(bus);
                3
            }
            0x6c => {
                let pointer = self.abs(bus);
                self.pc = self.read_word_bug(bus, pointer);
                5
            }
            0x20 => {
                let target = self.abs(bus);
                self.push_word(bus, self.pc.wrapping_sub(1));
                self.pc = target;
                6
            }

            // LDA
            0xa9 => {
                self.a = self.fetch(bus);
                self.set_zn(self.a);
                2
            }
            0xa5 => {
                let a = self.zp(bus);
                self.a = bus.read(a);
                self.set_zn(self.a);
                3
            }
            0xb5 => {
                let a = self.zpx(bus);
                self.a = bus.read(a);
                self.set_zn(self.a);
                4
            }
            0xad => {
                let a = self.abs(bus);
                self.a = bus.read(a);
                self.set_zn(self.a);
                4
            }
            0xbd => {
                let (a, p) = self.absx(bus);
                self.a = bus.read(a);
                self.set_zn(self.a);
                4 + p as u16
            }
            0xb9 => {
                let (a, p) = self.absy(bus);
                self.a = bus.read(a);
                self.set_zn(self.a);
                4 + p as u16
            }
            0xa1 => {
                let a = self.indx(bus);
                self.a = bus.read(a);
                self.set_zn(self.a);
                6
            }
            0xb1 => {
                let (a, p) = self.indy(bus);
                self.a = bus.read(a);
                self.set_zn(self.a);
                5 + p as u16
            }
            // LDX
            0xa2 => {
                self.x = self.fetch(bus);
                self.set_zn(self.x);
                2
            }
            0xa6 => {
                let a = self.zp(bus);
                self.x = bus.read(a);
                self.set_zn(self.x);
                3
            }
            0xb6 => {
                let a = self.zpy(bus);
                self.x = bus.read(a);
                self.set_zn(self.x);
                4
            }
            0xae => {
                let a = self.abs(bus);
                self.x = bus.read(a);
                self.set_zn(self.x);
                4
            }
            0xbe => {
                let (a, p) = self.absy(bus);
                self.x = bus.read(a);
                self.set_zn(self.x);
                4 + p as u16
            }
            // LDY
            0xa0 => {
                self.y = self.fetch(bus);
                self.set_zn(self.y);
                2
            }
            0xa4 => {
                let a = self.zp(bus);
                self.y = bus.read(a);
                self.set_zn(self.y);
                3
            }
            0xb4 => {
                let a = self.zpx(bus);
                self.y = bus.read(a);
                self.set_zn(self.y);
                4
            }
            0xac => {
                let a = self.abs(bus);
                self.y = bus.read(a);
                self.set_zn(self.y);
                4
            }
            0xbc => {
                let (a, p) = self.absx(bus);
                self.y = bus.read(a);
                self.set_zn(self.y);
                4 + p as u16
            }

            // LSR
            0x4a => {
                self.a = self.lsr(self.a);
                2
            }
            0x46 => {
                let a = self.zp(bus);
                let v = bus.read(a);
                let r = self.lsr(v);
                bus.write(a, r);
                5
            }
            0x56 => {
                let a = self.zpx(bus);
                let v = bus.read(a);
                let r = self.lsr(v);
                bus.write(a, r);
                6
            }
            0x4e => {
                let a = self.abs(bus);
                let v = bus.read(a);
                let r = self.lsr(v);
                bus.write(a, r);
                6
            }
            0x5e => {
                let (a, _) = self.absx(bus);
                let v = bus.read(a);
                let r = self.lsr(v);
                bus.write(a, r);
                7
            }

            0xea => 2,

            // ORA
            0x09 => {
                let v = self.fetch(bus);
                self.a |= v;
                self.set_zn(self.a);
                2
            }
            0x05 => {
                let a = self.zp(bus);
                self.a |= bus.read(a);
                self.set_zn(self.a);
                3
            }
            0x15 => {
                let a = self.zpx(bus);
                self.a |= bus.read(a);
                self.set_zn(self.a);
                4
            }
            0x0d => {
                let a = self.abs(bus);
                self.a |= bus.read(a);
                self.set_zn(self.a);
                4
            }
            0x1d => {
                let (a, p) = self.absx(bus);
                self.a |= bus.read(a);
                self.set_zn(self.a);
                4 + p as u16
            }
            0x19 => {
                let (a, p) = self.absy(bus);
                self.a |= bus.read(a);
                self.set_zn(self.a);
                4 + p as u16
            }
            0x01 => {
                let a = self.indx(bus);
                self.a |= bus.read(a);
                self.set_zn(self.a);
                6
            }
            0x11 => {
                let (a, p) = self.indy(bus);
                self.a |= bus.read(a);
                self.set_zn(self.a);
                5 + p as u16
            }

            // Stack
            0x48 => {
                self.push(bus, self.a);
                3
            }
            0x08 => {
                self.push(bus, self.status | B | U);
                3
            }
            0x68 => {
                self.a = self.pop(bus);
                self.set_zn(self.a);
                4
            }
            0x28 => {
                self.status = (self.pop(bus) | U) & !B;
                4
            }

            // ROL
            0x2a => {
                self.a = self.rol(self.a);
                2
            }
            0x26 => {
                let a = self.zp(bus);
                let v = bus.read(a);
                let r = self.rol(v);
                bus.write(a, r);
                5
            }
            0x36 => {
                let a = self.zpx(bus);
                let v = bus.read(a);
                let r = self.rol(v);
                bus.write(a, r);
                6
            }
            0x2e => {
                let a = self.abs(bus);
                let v = bus.read(a);
                let r = self.rol(v);
                bus.write(a, r);
                6
            }
            0x3e => {
                let (a, _) = self.absx(bus);
                let v = bus.read(a);
                let r = self.rol(v);
                bus.write(a, r);
                7
            }
            // ROR
            0x6a => {
                self.a = self.ror(self.a);
                2
            }
            0x66 => {
                let a = self.zp(bus);
                let v = bus.read(a);
                let r = self.ror(v);
                bus.write(a, r);
                5
            }
            0x76 => {
                let a = self.zpx(bus);
                let v = bus.read(a);
                let r = self.ror(v);
                bus.write(a, r);
                6
            }
            0x6e => {
                let a = self.abs(bus);
                let v = bus.read(a);
                let r = self.ror(v);
                bus.write(a, r);
                6
            }
            0x7e => {
                let (a, _) = self.absx(bus);
                let v = bus.read(a);
                let r = self.ror(v);
                bus.write(a, r);
                7
            }

            0x40 => {
                self.status = (self.pop(bus) | U) & !B;
                self.pc = self.pop_word(bus);
                6
            }
            0x60 => {
                self.pc = self.pop_word(bus).wrapping_add(1);
                6
            }

            // SBC
            0xe9 | 0xeb => {
                let v = self.fetch(bus);
                self.sbc(v);
                2
            }
            0xe5 => {
                let a = self.zp(bus);
                let v = bus.read(a);
                self.sbc(v);
                3
            }
            0xf5 => {
                let a = self.zpx(bus);
                let v = bus.read(a);
                self.sbc(v);
                4
            }
            0xed => {
                let a = self.abs(bus);
                let v = bus.read(a);
                self.sbc(v);
                4
            }
            0xfd => {
                let (a, p) = self.absx(bus);
                let v = bus.read(a);
                self.sbc(v);
                4 + p as u16
            }
            0xf9 => {
                let (a, p) = self.absy(bus);
                let v = bus.read(a);
                self.sbc(v);
                4 + p as u16
            }
            0xe1 => {
                let a = self.indx(bus);
                let v = bus.read(a);
                self.sbc(v);
                6
            }
            0xf1 => {
                let (a, p) = self.indy(bus);
                let v = bus.read(a);
                self.sbc(v);
                5 + p as u16
            }

            // STA/STX/STY
            0x85 => {
                let a = self.zp(bus);
                bus.write(a, self.a);
                3
            }
            0x95 => {
                let a = self.zpx(bus);
                bus.write(a, self.a);
                4
            }
            0x8d => {
                let a = self.abs(bus);
                bus.write(a, self.a);
                4
            }
            0x9d => {
                let (a, _) = self.absx(bus);
                bus.write(a, self.a);
                5
            }
            0x99 => {
                let (a, _) = self.absy(bus);
                bus.write(a, self.a);
                5
            }
            0x81 => {
                let a = self.indx(bus);
                bus.write(a, self.a);
                6
            }
            0x91 => {
                let (a, _) = self.indy(bus);
                bus.write(a, self.a);
                6
            }
            0x86 => {
                let a = self.zp(bus);
                bus.write(a, self.x);
                3
            }
            0x96 => {
                let a = self.zpy(bus);
                bus.write(a, self.x);
                4
            }
            0x8e => {
                let a = self.abs(bus);
                bus.write(a, self.x);
                4
            }
            0x84 => {
                let a = self.zp(bus);
                bus.write(a, self.y);
                3
            }
            0x94 => {
                let a = self.zpx(bus);
                bus.write(a, self.y);
                4
            }
            0x8c => {
                let a = self.abs(bus);
                bus.write(a, self.y);
                4
            }

            // Transfers
            0xaa => {
                self.x = self.a;
                self.set_zn(self.x);
                2
            }
            0xa8 => {
                self.y = self.a;
                self.set_zn(self.y);
                2
            }
            0xba => {
                self.x = self.sp;
                self.set_zn(self.x);
                2
            }
            0x8a => {
                self.a = self.x;
                self.set_zn(self.a);
                2
            }
            0x9a => {
                self.sp = self.x;
                2
            }
            0x98 => {
                self.a = self.y;
                self.set_zn(self.a);
                2
            }

            // Harmless unofficial NOP encodings frequently used by test/homebrew ROMs.
            0x1a | 0x3a | 0x5a | 0x7a | 0xda | 0xfa => 2,
            0x80 | 0x82 | 0x89 | 0xc2 | 0xe2 => {
                self.fetch(bus);
                2
            }
            0x04 | 0x44 | 0x64 => {
                self.fetch(bus);
                3
            }
            0x14 | 0x34 | 0x54 | 0x74 | 0xd4 | 0xf4 => {
                self.fetch(bus);
                4
            }
            0x0c => {
                self.abs(bus);
                4
            }
            0x1c | 0x3c | 0x5c | 0x7c | 0xdc | 0xfc => {
                let (_, p) = self.absx(bus);
                4 + p as u16
            }

            _ => {
                return Err(CpuError {
                    opcode,
                    address: opcode_address,
                });
            }
        };
        Ok(cycles)
    }

    fn fetch(&mut self, bus: &mut Bus) -> u8 {
        let value = bus.read(self.pc);
        self.pc = self.pc.wrapping_add(1);
        value
    }
    fn zp(&mut self, bus: &mut Bus) -> u16 {
        self.fetch(bus) as u16
    }
    fn zpx(&mut self, bus: &mut Bus) -> u16 {
        self.fetch(bus).wrapping_add(self.x) as u16
    }
    fn zpy(&mut self, bus: &mut Bus) -> u16 {
        self.fetch(bus).wrapping_add(self.y) as u16
    }
    fn abs(&mut self, bus: &mut Bus) -> u16 {
        let lo = self.fetch(bus) as u16;
        let hi = self.fetch(bus) as u16;
        (hi << 8) | lo
    }
    fn absx(&mut self, bus: &mut Bus) -> (u16, bool) {
        let base = self.abs(bus);
        let address = base.wrapping_add(self.x as u16);
        (address, base & 0xff00 != address & 0xff00)
    }
    fn absy(&mut self, bus: &mut Bus) -> (u16, bool) {
        let base = self.abs(bus);
        let address = base.wrapping_add(self.y as u16);
        (address, base & 0xff00 != address & 0xff00)
    }
    fn indx(&mut self, bus: &mut Bus) -> u16 {
        let pointer = self.fetch(bus).wrapping_add(self.x);
        let lo = bus.read(pointer as u16) as u16;
        let hi = bus.read(pointer.wrapping_add(1) as u16) as u16;
        (hi << 8) | lo
    }
    fn indy(&mut self, bus: &mut Bus) -> (u16, bool) {
        let pointer = self.fetch(bus);
        let lo = bus.read(pointer as u16) as u16;
        let hi = bus.read(pointer.wrapping_add(1) as u16) as u16;
        let base = (hi << 8) | lo;
        let address = base.wrapping_add(self.y as u16);
        (address, base & 0xff00 != address & 0xff00)
    }
    fn read_word(&mut self, bus: &mut Bus, address: u16) -> u16 {
        bus.read(address) as u16 | ((bus.read(address.wrapping_add(1)) as u16) << 8)
    }
    fn read_word_bug(&mut self, bus: &mut Bus, address: u16) -> u16 {
        let next = (address & 0xff00) | ((address.wrapping_add(1)) & 0x00ff);
        bus.read(address) as u16 | ((bus.read(next) as u16) << 8)
    }
    fn push(&mut self, bus: &mut Bus, value: u8) {
        bus.write(0x0100 | self.sp as u16, value);
        self.sp = self.sp.wrapping_sub(1);
    }
    fn pop(&mut self, bus: &mut Bus) -> u8 {
        self.sp = self.sp.wrapping_add(1);
        bus.read(0x0100 | self.sp as u16)
    }
    fn push_word(&mut self, bus: &mut Bus, value: u16) {
        self.push(bus, (value >> 8) as u8);
        self.push(bus, value as u8);
    }
    fn pop_word(&mut self, bus: &mut Bus) -> u16 {
        let lo = self.pop(bus) as u16;
        let hi = self.pop(bus) as u16;
        (hi << 8) | lo
    }

    fn flag(&self, flag: u8) -> bool {
        self.status & flag != 0
    }
    fn set_flag(&mut self, flag: u8, set: bool) {
        if set {
            self.status |= flag;
        } else {
            self.status &= !flag;
        }
    }
    fn set_zn(&mut self, value: u8) {
        self.set_flag(Z, value == 0);
        self.set_flag(N, value & 0x80 != 0);
    }
    fn adc(&mut self, value: u8) {
        let sum = self.a as u16 + value as u16 + self.flag(C) as u16;
        let result = sum as u8;
        self.set_flag(C, sum > 0xff);
        self.set_flag(V, (!(self.a ^ value) & (self.a ^ result) & 0x80) != 0);
        self.a = result;
        self.set_zn(self.a);
    }
    fn sbc(&mut self, value: u8) {
        self.adc(!value);
    }
    fn compare(&mut self, register: u8, value: u8) {
        self.set_flag(C, register >= value);
        self.set_zn(register.wrapping_sub(value));
    }
    fn bit(&mut self, value: u8) {
        self.set_flag(Z, self.a & value == 0);
        self.set_flag(V, value & 0x40 != 0);
        self.set_flag(N, value & 0x80 != 0);
    }
    fn asl(&mut self, value: u8) -> u8 {
        self.set_flag(C, value & 0x80 != 0);
        let r = value << 1;
        self.set_zn(r);
        r
    }
    fn lsr(&mut self, value: u8) -> u8 {
        self.set_flag(C, value & 1 != 0);
        let r = value >> 1;
        self.set_zn(r);
        r
    }
    fn rol(&mut self, value: u8) -> u8 {
        let carry = self.flag(C) as u8;
        self.set_flag(C, value & 0x80 != 0);
        let r = (value << 1) | carry;
        self.set_zn(r);
        r
    }
    fn ror(&mut self, value: u8) -> u8 {
        let carry = (self.flag(C) as u8) << 7;
        self.set_flag(C, value & 1 != 0);
        let r = (value >> 1) | carry;
        self.set_zn(r);
        r
    }
    fn branch(&mut self, bus: &mut Bus, condition: bool) -> u16 {
        let offset = self.fetch(bus) as i8;
        if !condition {
            return 2;
        }
        let old = self.pc;
        self.pc = self.pc.wrapping_add_signed(offset as i16);
        3 + (old & 0xff00 != self.pc & 0xff00) as u16
    }
    fn interrupt(&mut self, bus: &mut Bus, vector: u16, software: bool) {
        self.push_word(bus, self.pc);
        let mut flags = self.status | U;
        if software {
            flags |= B;
        } else {
            flags &= !B;
        }
        self.push(bus, flags);
        self.set_flag(I, true);
        self.pc = self.read_word(bus, vector);
    }
}

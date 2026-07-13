//! Platform-independent NES emulation core.
//!
//! The core deliberately knows nothing about windows, keyboards, audio devices,
//! files, or wall-clock time. Front ends provide those concerns around `Nes`.

pub mod apu;
pub mod bus;
pub mod cartridge;
pub mod controller;
pub mod cpu;
pub mod emulator;
pub mod ppu;

pub use cartridge::{Cartridge, CartridgeError, Mirroring};
pub use controller::{Button, Controller};
pub use emulator::{EmulationError, Nes};
pub use ppu::{FRAME_HEIGHT, FRAME_WIDTH, Frame, PpuState};

/// NTSC NES master-derived CPU frequency.
pub const CPU_CLOCK_HZ: u32 = 1_789_773;
pub const NTSC_FRAME_RATE: f64 = 60.098_813_897_440_5;

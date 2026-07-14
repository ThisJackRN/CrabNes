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
pub mod timing;

pub use apu::{ApuChannel, ApuState};
pub use cartridge::{Cartridge, CartridgeError, Mirroring};
pub use controller::{Button, Controller};
pub use emulator::{EmulationError, MemoryImage, MemorySpace, Nes, SAVE_STATE_VERSION, StateError};
pub use ppu::{
    FRAME_HEIGHT, FRAME_WIDTH, Frame, NTSC_2C02_PALETTE, OutputPalette, PpuState, RGB_2C03_PALETTE,
};
pub use timing::{NTSC_CPU_CLOCK_HZ, NTSC_FRAME_RATE, PAL_CPU_CLOCK_HZ, PAL_FRAME_RATE, Region};

/// Backwards-compatible alias for the NTSC CPU frequency.
pub const CPU_CLOCK_HZ: u32 = NTSC_CPU_CLOCK_HZ;

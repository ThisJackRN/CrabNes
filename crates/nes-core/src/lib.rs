//! Platform-independent NES emulation core.
//!
//! The core deliberately knows nothing about windows, keyboards, audio devices,
//! files, or wall-clock time. Front ends provide those concerns around `Nes`.

pub mod apu;
pub mod bus;
pub mod cartridge;
pub mod cheat;
pub mod controller;
pub mod cpu;
pub mod emulator;
mod fceux_state;
pub mod ppu;
pub mod timing;

pub use apu::{ApuChannel, ApuState};
pub use cartridge::{Cartridge, CartridgeError, Mirroring};
pub use cheat::{Cheat, CheatError};
pub use controller::{Button, Controller};
pub use emulator::{
    CheatActivity, EmulationError, MemoryImage, MemorySpace, Nes, SAVE_STATE_VERSION, StateError,
};
pub use fceux_state::FceuxStateError;
pub use ppu::{
    FRAME_HEIGHT, FRAME_WIDTH, Frame, NTSC_2C02_PALETTE, OutputPalette, PpuState, RGB_2C03_PALETTE,
    RGB_2C04_0004_PALETTE,
};
pub use timing::{NTSC_CPU_CLOCK_HZ, NTSC_FRAME_RATE, PAL_CPU_CLOCK_HZ, PAL_FRAME_RATE, Region};

/// Backwards-compatible alias for the NTSC CPU frequency.
pub const CPU_CLOCK_HZ: u32 = NTSC_CPU_CLOCK_HZ;

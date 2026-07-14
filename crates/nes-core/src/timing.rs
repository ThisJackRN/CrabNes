use serde::{Deserialize, Serialize};

/// Hardware timing family selected by the ROM header.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum Region {
    #[default]
    Ntsc,
    Pal,
}

impl Region {
    pub const fn cpu_clock_hz(self) -> u32 {
        match self {
            Self::Ntsc => NTSC_CPU_CLOCK_HZ,
            Self::Pal => PAL_CPU_CLOCK_HZ,
        }
    }

    pub const fn frame_rate(self) -> f64 {
        match self {
            Self::Ntsc => NTSC_FRAME_RATE,
            Self::Pal => PAL_FRAME_RATE,
        }
    }

    pub(crate) const fn ppu_scanlines(self) -> i16 {
        match self {
            Self::Ntsc => 262,
            Self::Pal => 312,
        }
    }
}

/// NTSC RP2A03 CPU frequency, rounded to the nearest whole hertz.
pub const NTSC_CPU_CLOCK_HZ: u32 = 1_789_773;
/// PAL RP2A07 CPU frequency, rounded to the nearest whole hertz.
pub const PAL_CPU_CLOCK_HZ: u32 = 1_662_607;
pub const NTSC_FRAME_RATE: f64 = 60.098_813_897_440_5;
pub const PAL_FRAME_RATE: f64 = 50.006_978_908_188_586;

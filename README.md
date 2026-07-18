# CrabNes

[![Windows build](https://github.com/ThisJackRN/CrabNes/actions/workflows/windows-build.yml/badge.svg)](https://github.com/ThisJackRN/CrabNes/actions/workflows/windows-build.yml)

A NES and Famicom Disk System emulator written from scratch in Rust.

**141/141 on [AccuracyCoin](https://github.com/100thCoin/AccuracyCoin)** — the full NTSC NES accuracy suite passes.

<p align="center">
  <img src="docs/accuracycoin-141-of-141-v2.png" alt="AccuracyCoin results showing all 141 tests passing">
</p>

## Download

Grab the latest `CrabNes-windows-x64` artifact from the [Windows build workflow](https://github.com/ThisJackRN/CrabNes/actions/workflows/windows-build.yml), or [build from source](#build-from-source).

## Features

**Emulation**
- Cycle-accurate NTSC and PAL CPU, PPU, and APU with unofficial opcodes
- Famicom Disk System with multi-disk support and wavetable audio
- Mappers: NROM, MMC1, UxROM, CNROM, MMC3, MMC5, AxROM, MMC2, MMC4, Namco 163, VRC2/4, VRC6, VRC7, FME-7, Vs. System
- Expansion audio: FDS, Sunsoft 5B, VRC6, VRC7, Namco 163, MMC5

**Playback**
- Native low-latency Windows audio with per-channel mixing
- Keyboard and gamepad support for two players
- Optional CRT rendering and custom palettes
- Configurable overscan cropping

**Library & Saves**
- Searchable ROM library with artwork and compatibility estimates
- Versioned save states with screenshots
- LZ4-compressed rewind (up to 10 minutes)
- Battery saves beside the ROM

**Tools**
- TAS editor with recording, rerecording, frame editing, seeking, and checkpoints
- FCEUX `.fm2` and BizHawk `.bk2` movie import
- Debugger, hex editor, and cheat code manager (Game Genie + raw patches)

**Extras**
- RetroAchievements integration (sign-in, badges, progress, unlock archive)
- Play profiles: Standard, Speedrun, and Achievement modes
- Speedrun/Achievement profiles lock out emulator assists for fair play

## Controls

| Key | Action |
|---|---|
| Arrow keys | P1 D-pad |
| Z / X | P1 A / B |
| Shift / Enter | P1 Select / Start |
| I / J / K / L | P2 D-pad |
| C / V | P2 A / B |
| Q / E | P2 Select / Start |
| Ctrl+O | Open ROM |
| Space | Pause |
| R | Reset |
| P | Power |
| 6 | FDS eject/insert |
| F5 / F8 | Quick save / load |
| N | Frame advance |
| Backspace (hold) | Rewind |
| Tab (hold) | Fast-forward |
| F1 / F2 | Debugger / hex editor |
| F11 | Fullscreen |
| F12 | Screenshot |

All bindings are remappable in **Settings > Input**.

## Compatibility

| | |
|---|---|
| **Region** | NTSC and PAL |
| **Format** | iNES 1.0, NES 2.0, `.fds` (headered and headerless) |
| **Mappers** | 0, 1, 2, 3, 4, 5, 7, 9, 10, 19, 20, 21–23, 24–26, 69, 85, 99 |
| **Expansion audio** | FDS, Sunsoft 5B, VRC6, VRC7, Namco 163, MMC5 |
| **Platform** | Windows x64 |

## Famicom Disk System

FDS games need a `.fds` disk image and the FDS BIOS (`disksys.rom`, 8 KiB). Set the BIOS path under **Settings > Paths > Choose FDS BIOS…**. CrabNes also looks for `disksys.rom` in the working directory or beside the `.fds` file.

Press `6` to eject/insert disks. Multi-side images cycle through sides automatically.

## Play Profiles

| Profile | Assists | Use case |
|---|---|---|
| **Standard** | All tools available | Normal play, debugging, TAS |
| **Speedrun** | Locked out | Clean real-time runs |
| **Achievements** | Locked out | RetroAchievements play |

Speedrun and Achievement profiles disable rewind, save states, frame advance, TAS tools, and the debugger.

## RetroAchievements

CrabNes vendors rcheevos 12.3.0. Sign-in, game detection, badges, progress tracking, and unlock notifications all work. CrabNes is not yet an approved client, so hardcore unlocks are not awarded by the service.

## Build from source

Requires the [stable Rust toolchain](https://www.rust-lang.org/tools/install) and Visual Studio C++ Build Tools.

```powershell
git clone https://github.com/ThisJackRN/CrabNes.git
cd CrabNes
cargo run --release -p nes-ui
```

Pass a ROM path to open it directly:

```powershell
cargo run --release -p nes-ui -- path\to\game.nes
```

Headless runner for testing and screenshots:

```powershell
cargo run --release -p nes-cli -- path\to\game.nes --frames 120 --screenshot frame.png
```

## Documentation

- [TAS movie format](docs/TAS_FORMAT.md)
- [TAS Control View guide](docs/TAS_CONTROL_VIEW.md)
- [CRT filters](docs/CRT_FILTER.md)
- [Custom palettes](docs/CUSTOM_PALETTES.md)
- [Test ROM runner](docs/TEST_ROMS.md)
- [Third-party licenses](THIRD_PARTY_NOTICES.md)

## Project layout

```
crates/
  nes-core/                    Emulation core (no platform dependencies)
  nes-audio-native/            Native audio output (miniaudio)
  nes-achievements-native/     RetroAchievements (rcheevos wrapper)
  nes-cli/                     Headless runner and test tools
  nes-ui/                      CrabNes desktop application
```

## License

MIT — see [LICENSE](LICENSE). Third-party attributions in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

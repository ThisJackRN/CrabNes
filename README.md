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
- Switchable **FCEUX-compatible controller timing** for exact FCEUX movie playback (see [below](#fceux-compatibility-mode))

**Playback**
- Native low-latency Windows audio with per-channel mixing
- Keyboard and gamepad support for two players
- Optional CRT rendering, RGB/NTSC/custom palettes, and configurable overscan cropping

**Library & Saves**
- Searchable ROM library with artwork and compatibility estimates
- Versioned save states with screenshots
- LZ4-compressed rewind (up to 10 minutes)
- Battery saves beside the ROM

**Tools**
- TAS editor with recording, rerecording, frame editing, seeking, and checkpoints
- FCEUX `.fm2` and BizHawk `.bk2`/input-log movie import via the TAS Control View
- Debugger, live-mode hex editor, and a cheat code manager (Game Genie + raw patches) with a live activity view

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
| P / Ctrl+P | Power toggle / power cycle |
| 6 | FDS eject/insert |
| F5 / F8 | Quick save / load |
| Shift+F5 | Save states window |
| N | Frame advance |
| 0 | Reset speed to 1x |
| Backspace (hold) | Rewind |
| Tab (hold) | Fast-forward |
| F1 / F2 | Debugger / hex editor |
| F11 | Fullscreen |
| F12 | Screenshot |

All bindings are remappable in **Settings > Input**, including a second gamepad-only binding per action.

## Menus

| Menu | Contents |
|---|---|
| **File** | Open ROM, recent games |
| **Emulation** | Reset, power, speed, region overrides |
| **View** | Fullscreen, presentation toggles |
| **Config** | Settings, Input Configuration, Audio / Video, and the **Tools** submenu (Save States, Rewind & Speed, Cheat Codes, TAS Editor, TAS Control View) |
| **Game / Library** | Switch between the running game and the ROM library |

**Settings** is organized into categories: General, Video, Audio, Input, Emulation, Paths, Save States, TAS, and Debugging. Most categories also show a **Per-game overrides** panel (volume, mute, speed, and palette) scoped to whichever ROM is currently loaded.

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

## Palettes

**Tools > Audio / Video** (or **Settings > Video**) chooses the color palette: NTSC 2C02 (default), RGB 2C03 / PlayChoice-10, RGB 2C04-0004 (Vs. System), or a custom imported palette. See [Custom palettes](docs/CUSTOM_PALETTES.md) for supported file formats.

Nintendo Vs. System (mapper 99) ROMs used a scrambled-palette RGB PPU rather than a home console's composite one, so the wrong chip renders visibly wrong colors, not just different ones. Vs. System games therefore always render with RGB 2C04-0004 unless that specific game has an explicit palette override — regardless of the global setting used by every other ROM. Override a Vs. game's palette in **Per-game overrides > Override palette** (shown in both the Settings window and the Audio / Video window).

## Cheat codes

**Tools > Cheat Codes** manages a named, per-game list of codes. Each entry accepts:

- Standard 6- or 8-letter **Game Genie** codes (with or without hyphens)
- **Raw CPU patches** — `ADDRESS:VALUE` or `ADDRESS?COMPARE:VALUE`, in hex

Enabled codes apply immediately, persist across reset/rewind/save-state loads, and are automatically disabled in Speedrun and Achievement profiles. The window also shows **live activity**: which codes are actively patching a byte right now versus waiting on an unmet compare value, and a running hit counter — and it can jump straight to the patched address in the hex editor.

## Debugger and hex editor

`F1` opens the debugger (CPU/PPU state, frame stepping). `F2` opens the hex editor, which can browse CPU RAM, the full CPU bus (post-cheat, as the running program sees it), PPU nametables, palette RAM, OAM, PRG ROM, and CHR. **Live mode** keeps the game running while the hex editor is open instead of pausing, so RAM and cheat activity update in real time; toggle it in the editor itself or under **Settings > Debugging**.

## Save states and rewind

Each game gets ten save-state slots (configurable) with screenshots, managed from **Tools > Save States**. Rewind keeps an LZ4-compressed ring of full-machine snapshots — up to 10 minutes by default — accessed by holding Backspace or from **Tools > Rewind & Speed**.

## TAS tools

The **TAS Editor** (`Tools > TAS Editor`) records both controllers once per emulated frame, with power-on/reset/embedded-state starting conditions, read-only playback, held input, insert/delete/range editing, bookmarks, rerecord counts, and deterministic seeking through cached checkpoints (with automatic desync detection and recovery). Game Genie/raw cheat codes enabled when a recording starts are locked into that movie, the same way a physical device would be — playback always reapplies them.

The **TAS Control View** (`Tools > TAS Control View`) imports and converts foreign movie formats — FCEUX `.fm2`, BizHawk `.bk2` and extracted input logs — into CrabNes's native format, including embedded FCEUX savestates for ACE/glitch movies.

Native `.tas` files are readable text and include the ROM SHA-256. See the [TAS movie format](docs/TAS_FORMAT.md) and [TAS Control View guide](docs/TAS_CONTROL_VIEW.md).

### FCEUX compatibility mode

Real NES hardware corrupts a controller read when a DMC or OAM DMA cycle overlaps it; games that use DMC audio (Super Mario Bros. 3, for example) defend against this by re-reading the controller, which costs CPU time and can shift lag frames. CrabNes emulates this corruption by default, and AccuracyCoin's controller-clocking tests verify it.

FCEUX never emulated that corruption, so `.fm2` movies were recorded without it and can desync against hardware-accurate timing. CrabNes handles this automatically:

- **Normal play, native recordings, Speedrun/Achievement profiles** always use hardware-accurate timing.
- **FM2 conversions** default to FCEUX-compatible timing (toggleable in the TAS Control View). The setting is written into the converted `.tas` file and reapplied on every playback/seek, so it stays correct without further thought.
- **Settings > Emulation > Advanced accuracy** exposes the same switch for Standard-mode play in general, for experiments outside movie playback. It has no effect on a movie that is actively playing — the movie's own recorded setting always wins.

## Play profiles

| Profile | Assists | Use case |
|---|---|---|
| **Standard** | All tools available | Normal play, debugging, TAS |
| **Speedrun** | Locked out | Clean real-time runs |
| **Achievements** | Locked out | RetroAchievements play |

Speedrun and Achievement profiles disable rewind, save states, frame advance, TAS tools, cheats, and the debugger.

## RetroAchievements

CrabNes vendors rcheevos 12.3.0. Sign-in, game detection, badges, progress tracking, and unlock notifications all work. CrabNes is not yet an approved client, so hardcore unlocks are not awarded by the service.

## Settings and existing data

CrabNes stores user data under:

```text
%LOCALAPPDATA%\CrabNes\
  settings.json                 Global settings and play profile
  per-game-settings.json        ROM-specific presentation overrides
  library.json                  Library metadata and recent games
  library-covers\               Cached automatic and copied custom artwork
  achievement-archive.json      Local RetroAchievements unlock history
  palettes\                     Imported custom palettes
  states\<rom-hash>\            Save-state slots and previews
  tas\<rom-hash>\               Default TAS folder
```

Battery saves and screenshots remain beside the ROM.

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

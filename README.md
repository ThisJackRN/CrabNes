# CrabNes

[![Windows build](https://github.com/ThisJackRN/CrabNes/actions/workflows/windows-build.yml/badge.svg)](https://github.com/ThisJackRN/CrabNes/actions/workflows/windows-build.yml)

CrabNes is a from-scratch NES emulator written in Rust. It combines a
deterministic emulation core with a native Windows desktop interface, responsive
audio, long smooth rewind, TAS tools, speedrun-safe play profiles, and
RetroAchievements support.

> [!IMPORTANT]
> CrabNes supports the major Nintendo, Konami, Sunsoft, and Namco mapper families
> listed in the compatibility table below. Accuracy work is ongoing. No commercial ROMs are included;
> use legally obtained games or homebrew.

## Measured emulation accuracy

**68.8% — 97 of 141 [AccuracyCoin](https://github.com/100thCoin/AccuracyCoin) tests passing.**

This is CrabNes's current automated hardware-conformance test pass rate, not a
claim that every NES game is exactly 68.8% accurate. Compatibility varies by
game, mapper, and the hardware behavior it relies on. The score is updated only
when it has been reproduced with the headless runner:

```powershell
cargo run --release -p nes-cli -- AccuracyCoin.nes --frames 5000 --press-start-at 120 --accuracycoin-report
```

AccuracyCoin targets the RP2A03G CPU/APU and RP2C02G PPU used by NTSC NES
hardware. PAL support is tested separately because it uses different timing.

## Download for Windows

Automated 64-bit Windows builds are available from GitHub Actions:

1. Open the [Windows build workflow](https://github.com/ThisJackRN/CrabNes/actions/workflows/windows-build.yml).
2. Select the newest successful run.
3. Download the `CrabNes-windows-x64` artifact.
4. Extract it and run `CrabNes.exe`.

Artifacts are retained by GitHub for 30 days. If no artifact is available yet,
run the workflow manually or [build from source](#build-from-source).

## What CrabNes includes

- Cycle-driven NTSC and PAL CPU, PPU, and APU emulation, including stable
  unofficial NMOS 6502 opcodes and observable dummy bus accesses.
- Cartridge IRQs, banked RAM/ROM, dynamic mirroring, and expansion audio.
- Native low-latency Windows audio with per-channel controls.
- Keyboard and hot-pluggable gamepad support for two players.
- Searchable ROM library with custom titles, automatic RetroAchievements
  artwork, user-selected artwork overrides, and a per-ROM accuracy estimate
  that explains mapper and timing confidence.
- Versioned save states with screenshots and ROM validation.
- LZ4-compressed rewind: two minutes by default, configurable up to ten minutes.
- TAS recording, playback, rerecording, frame editing, seeking, and checkpoints.
- FCEUX `.fm2` and BizHawk `.bk2`/`Input Log.txt` movie import.
- Debugger, guarded hex editor, custom palettes, and optional CRT rendering.
- Standard, Speedrun, and Achievement play profiles.
- RetroAchievements sign-in, badge artwork, progress, unlock archive, and
  animated in-game notifications.

## Play profiles

Choose a profile under **Settings > General**.

| Profile | Intended use | Emulator assists |
|---|---|---|
| Standard | Normal play, debugging, and TAS work | Available |
| Speedrun | Clean real-time runs at normal speed | Disabled |
| Achievements | RetroAchievements play at normal speed | Disabled |

Speedrun and Achievement profiles lock the emulator to 1x and remove pause,
frame advance, rewind, save states, TAS tools, debugger/hex editing, impossible
D-pad combinations, and their hotkeys. Reset, power controls, input mapping,
screenshots, and presentation settings remain available.

### RetroAchievements status

CrabNes vendors the official rcheevos 12.3.0 client. Networking and badge image
loading run off the emulation thread, while achievement evaluation uses a
side-effect-free memory snapshot once per emulated frame.

CrabNes is not yet an approved RetroAchievements client. Normal account sign-in,
game identification, sets, badges, progress, and unlock UI work, but the service
does not award hardcore unlocks to an unknown emulator. The Achievements window
pins client and game-version limitations as warnings instead of presenting them
as game achievements or unlock notifications.

On startup and whenever the library is refreshed, CrabNes scans new ROMs and
caches artwork for games recognized by RetroAchievements. This happens in the
background without launching the game or signing in. Use **Set Custom
Artwork…** from a library entry's menu to override it; removing the custom image
restores the cached one.

## Compatibility

| Area | Current support |
|---|---|
| Region | NTSC and PAL; standard Europe/Australia/PAL filename tags correct legacy ROMs with missing timing flags; multi-region NES 2.0 images default to NTSC |
| ROM format | iNES 1.0 and NES 2.0 for supported boards |
| Mapper | 0 NROM; 1 MMC1; 2 UxROM; 3 CNROM; 4 MMC3; 5 MMC5; 7 AxROM; 9 MMC2; 10 MMC4; 19 Namco 163; 21/22/23/25 VRC2/VRC4; 24/26 VRC6; 69 FME-7/5B; 85 VRC7 |
| Expansion audio | Sunsoft 5B, VRC6, Namco 163, MMC5, and VRC7 FM |
| Desktop | Windows x64 |
| Controllers | Two NES controllers through keyboard and gamepads |
| Battery RAM | `.sav` beside the ROM |
| Save states | Versioned, validated, and separated by ROM hash |

The PPU models rendering-time VRAM increments, palette bus behavior, VBlank/NMI
suppression, OAM access restrictions, grayscale masking, and timed sprite
overflow, but is not yet dot-perfect for every sprite-evaluation and
fetch-pipeline quirk. MMC5 extended attributes and vertical split rendering,
exact VRC7 FM operator/envelope behavior, and unusual board variants still need
accuracy work. Dendy timing, light guns, Four Score, and cartridge families
outside the table are not implemented yet.

## Controls

All gameplay bindings can be changed in **Settings > Input**.

| Default key | Action |
|---|---|
| Arrow keys | Player 1 D-pad |
| Z / X | Player 1 A / B |
| Shift / Enter | Player 1 Select / Start |
| I / J / K / L | Player 2 Up / Left / Down / Right |
| C / V | Player 2 A / B |
| Q / E | Player 2 Select / Start |
| Ctrl+O | Open ROM |
| Space | Pause or resume |
| R | Reset |
| P | Power off or on |
| Ctrl+P | Power cycle |
| F5 / F8 | Quick save / quick load |
| N | Advance one frame |
| Backspace (hold) | Rewind |
| Tab (hold) | 4x fast-forward |
| Num0 | Return to 1x speed |
| F1 / F2 | Debugger / hex editor |
| F11 | Fullscreen |
| F12 | Screenshot |

Assist hotkeys are ignored in Speedrun and Achievement profiles. Hotkeys are
also ignored while typing in a text field.

## Save states and rewind

Each game has ten save-state slots by default. States include the full CPU, PPU,
APU, mapper, controller, DMA, interrupt, power, timing, and framebuffer state.
They carry a ROM hash and format version, so incompatible states are rejected
before they can alter the running console.

Rewind stores periodic full-machine snapshots in a bounded, LZ4-compressed ring.
Compression happens on a background worker, and reverse playback uses a
drift-free 60 Hz schedule. Releasing Backspace resumes play when appropriate.

## TAS tools

The TAS editor records both controllers once per emulated frame. It supports
power-on, reset, and embedded-state starting conditions; read-only playback;
held input; insertion/deletion; range editing; bookmarks; rerecord counts; and
deterministic seeking through cached checkpoints.

Native `.tas` files are readable text and include the ROM SHA-256. CrabNes still
accepts movies created before the rename with the legacy emulator identifier.
See the [TAS format specification](docs/TAS_FORMAT.md) and
[TAS Control View guide](docs/TAS_CONTROL_VIEW.md).

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
  states\<rom-hash>\             Save-state slots and previews
  tas\<rom-hash>\                Default TAS folder
```

Battery saves and screenshots remain beside the ROM.

## Build from source

Install the [stable Rust toolchain](https://www.rust-lang.org/tools/install).
Windows builds may also require the Visual Studio C++ Build Tools because the
native audio backend and rcheevos runtime include C code.

```powershell
git clone https://github.com/ThisJackRN/CrabNes.git
cd CrabNes
cargo test --workspace --locked
cargo run --release -p nes-ui --locked
```

You can pass a ROM path on the command line:

```powershell
cargo run --release -p nes-ui --locked -- path\to\game.nes
```

The included `Play CrabNes.bat` launcher runs the optimized desktop application.
The headless runner can be used for smoke tests and screenshots:

```powershell
cargo run -p nes-cli --locked -- path\to\game.nes --frames 120 --screenshot frame.png
```

Developers can run individual ROMs or recursive directories that use the
standard blargg `$6000` pass/fail protocol:

```powershell
cargo run -p nes-cli --bin crabnes-test-rom --locked -- path\to\test-roms
```

See the [test-ROM runner guide](docs/TEST_ROMS.md) for limits, exit behavior, and
the current cycle-scheduling accuracy boundary.

## Documentation

- [TAS movie format](docs/TAS_FORMAT.md)
- [TAS Control View and external movie conversion](docs/TAS_CONTROL_VIEW.md)
- [CRT filters](docs/CRT_FILTER.md)
- [Custom palettes](docs/CUSTOM_PALETTES.md)
- [Test ROM runner and timing status](docs/TEST_ROMS.md)
- [Third-party licenses and acknowledgements](THIRD_PARTY_NOTICES.md)

## Project layout

```text
crates/
  nes-core/                    Platform-independent emulation core
  nes-audio-native/            Native miniaudio output
  nes-achievements-native/     Safe Rust wrapper around vendored rcheevos
  nes-cli/                     Headless frame and test-ROM runners
  nes-ui/                      CrabNes desktop application
```

The core contains no window, input-device, filesystem, audio-device, or
wall-clock dependencies. Front ends decide how much emulated work to request;
they do not act as the emulation clock.

## Development checks

```powershell
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release --workspace --locked
```

## License

CrabNes is available under the [MIT License](LICENSE). Vendored libraries,
adapted permissive code, and interoperability references are documented in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

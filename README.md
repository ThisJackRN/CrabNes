# My Own NES Emulator

[![Windows build](https://github.com/ThisJackRN/MyNesEmulator/actions/workflows/windows-build.yml/badge.svg)](https://github.com/ThisJackRN/MyNesEmulator/actions/workflows/windows-build.yml)

A from-scratch NES emulator in Rust with a platform-independent emulation core,
a native Windows audio backend, a headless CLI, and a desktop front end. The
current cartridge implementation supports iNES 1.0 Mapper 0 (NROM-128 and
NROM-256) games.

The core owns all emulated state and timing. The desktop UI uses public core APIs
for save states, rewind, TAS playback, and side-effect-free debug memory access;
keyboard events, file paths, audio devices, and wall-clock scheduling stay out of
`nes-core`.

Highlights include:

- Cycle-driven NTSC CPU, PPU, and APU emulation with native audio output
- A searchable ROM library with custom game titles and cover images
- Versioned per-game save states, screenshot previews, quick save/load, and rewind
- Deterministic TAS recording, playback, editing, frame advance, and FM2 import
- CPU/PPU memory tools, a debugger, custom palettes, and configurable CRT filters

No commercial games or copyrighted ROM images are included. Use legally obtained
or homebrew ROMs.

## Download the Windows build

Ready-to-run 64-bit Windows builds are produced automatically by GitHub Actions:

1. Open the [Windows build workflow](https://github.com/ThisJackRN/MyNesEmulator/actions/workflows/windows-build.yml).
2. Select the newest successful run (the one with a green check mark).
3. Download `MyOwnNesEmulator-windows-x64` from the **Artifacts** section.
4. Extract the ZIP and run `nes-ui.exe`.

The artifact is retained for 30 days. If there is no current artifact, use **Run
workflow** on the Actions page or build the emulator from source. Builds run for
pushes and pull requests, and each build runs the complete Rust test suite first.

## Build and run from source

Install the [stable Rust toolchain](https://www.rust-lang.org/tools/install). The
desktop build currently targets 64-bit Windows; the MSVC Rust toolchain may also
require the Visual Studio C++ Build Tools.

Test the workspace and start the desktop application:

```powershell
cargo test --workspace --locked
cargo run --release -p nes-ui --locked
```

Starting without a ROM opens the Library page. A ROM can also be supplied on the
command line:

```powershell
cargo run --release -p nes-ui --locked -- path\to\game.nes
```

The optimized executable is written to `target\release\nes-ui.exe`. The included
`Play NES.bat` launcher starts the same desktop application. The headless CLI is
useful for automated smoke tests and screenshots:

```powershell
cargo run -p nes-cli --locked -- path\to\game.nes --frames 120 --screenshot frame.png
```

## Compatibility status

| Area | Current support |
|---|---|
| Region | NTSC |
| ROM container | iNES 1.0 |
| Cartridge mapper | Mapper 0 / NROM-128 / NROM-256 |
| Desktop platform | Windows x64 |
| Controllers | Two standard NES controllers through remappable keyboard input |
| Save states | Versioned and separated by ROM hash |

Mapper support is intentionally limited for now; see [Current
limitations](#current-limitations) before reporting a game-specific problem.

## Documentation

- [TAS movie format](docs/TAS_FORMAT.md)
- [TAS Control View and external movie conversion](docs/TAS_CONTROL_VIEW.md)
- [CRT filters](docs/CRT_FILTER.md)
- [Custom palettes](docs/CUSTOM_PALETTES.md)
- [Third-party licenses and notices](THIRD_PARTY_NOTICES.md)

## Keyboard shortcuts

| Key | Action |
|---|---|
| Arrow keys | D-pad (remappable) |
| Z / X | A / B (remappable) |
| Enter / Shift | Start / Select (remappable) |
| I/J/K/L | Player 2 Up/Left/Down/Right (remappable) |
| C / V | Player 2 A / B (remappable) |
| E / Q | Player 2 Start / Select (remappable) |
| Ctrl+O | Open ROM |
| Space | Pause or resume |
| R | Reset |
| P | Power off or on |
| Ctrl+P | Power cycle |
| F5 | Quick-save selected slot |
| F8 | Quick-load selected slot |
| Shift+F5 | Open Save States |
| N | Advance one frame and pause |
| Hold Advance button | Continuously frame-advance at roughly NTSC frame rate |
| Backspace (hold) | Rewind |
| Tab (hold) | 4x fast-forward; audio is muted |
| Num0 | Return to 1x speed |
| F1 | Open debugger |
| F2 | Open hex editor and pause |
| F11 | Toggle fullscreen |
| F12 | Save a PNG in a `screenshots` folder beside the ROM |

Hotkeys are ignored while typing into a text field so editing an address, search,
or TAS value does not accidentally control the console.

## Desktop features

### ROM library and recent games

The dedicated Library page combines games previously opened by the user with
supported `.nes` files found in the configured ROM folder. A persistent Library
tab opens a compact cover/title card view with Play and a `...` menu. Each game can
have a custom library title and a PNG, JPEG, WebP, or BMP cover image. Selected
images are validated and copied into the application data folder. The menu can
also remove a game from the library without deleting its ROM file. Search,
title/recent sorting, refresh, path and ROM de-duplication, and safe handling of
missing or invalid files remain available.

The default ROM folder is `%USERPROFILE%\Documents\NES ROMs`. Choose another
folder on the Library page or under Settings > Paths. The selection persists.
The old `recent-roms.txt` list is imported once when upgrading from an earlier
build.

### Save states and rewind

Each game has 10 slots by default (configurable from 1 through 20). A slot shows
its local creation time and an RGB screenshot preview. F5 and F8 quick-save and
quick-load the selected slot. States are versioned and include the ROM hash, so a
state from a different game or incompatible emulator version is rejected before
it can alter the machine.

The core snapshot includes CPU registers/internal state, CPU RAM, PPU state,
nametables/palette/OAM/framebuffer, APU channels/sequencer/filter/resampler state,
mapper RAM/CHR RAM, controllers, DMA, open bus, CPU timing, power state, and
pending interrupt state. Presentation audio already queued to the host is cleared
after loading so old samples cannot pop or play after the restored frame.

Rewind uses the same core snapshot API in a bounded in-memory ring. The rewind
duration and snapshot interval are configurable. TAS position and lag-tool
counters are restored with each rewind point, which permits deterministic
rerecording after rewinding. Rewinding during a live TAS recording is destructive:
the restored frame and all later recorded inputs are removed, future checkpoints
are invalidated, and the movie's rerecord count is incremented.

### TAS tools

The TAS editor records complete Player 1 and Player 2 controller states once per
emulated frame. Movies can begin from deterministic power-on, reset, or an
embedded save state. Playback ignores normal gameplay input while pause, frame
advance, seek, save/load, and stop hotkeys remain active. Live recording is locked
to 1x; playback can use the normal speed controls.

The virtualized timeline shows every frame and both controllers' A/B/Select/Start/
D-pad states. Clicking a row selects it without moving the emulator. A stable
selected-frame panel provides large input toggles, previous/next row navigation,
copy-previous, and clear controls. Range clearing/filling, insertion, deletion,
duplication, and internal copy/paste are also available. Manual editing pauses the
emulator and invalidates later checkpoints but never seeks automatically; `Seek
selected` enters non-destructive preview mode, and `Rerecord from here` explicitly
switches back to live-input recording. Frame advance follows the next movie row;
the timeline includes a visible `next new frame / end of movie` row.

The TAS playhead is always the next input frame to execute. Pausing or frame
advancing selects that exact frame, including the editable end row. Writing input
to the end row creates the new current frame; it never modifies the frame that
just finished. Selecting a row only moves the editor highlight and never scrolls
or seeks behind the user's back. `Seek selected` explicitly moves and pauses the
machine; writing input to a non-current selected row also aligns the machine there
so the edit can be previewed safely.

Frame advance previews recorded rows until it reaches the end row, then
automatically continues recording. Each click or repeat therefore appends a new
row, including blank-input frames, instead of leaving the movie inactive. A
read-only movie remains protected and stops at its end.

The GUI controller buttons also act as a held-input latch. A selected button is
combined with live keyboard input and written into every new or rerecorded frame
while frame advance is held. The end-row buttons remain visibly selected until
the user unselects them; clearing the latch releases the buttons on subsequent
frames.

Bookmarks link directly to marked frames. Previous, Next, Seek selected, rewind,
and held frame advance move through the movie. Automatic full-machine checkpoints
default to every 300 frames and are configurable under Settings > TAS. Seeking
loads the nearest earlier checkpoint, replays recorded inputs to the target,
pauses there, and clears queued presentation audio. Optional checkpoint SHA-256
values detect state divergence during later playback where reference hashes exist.
The frame-advance control advances once on click and repeats at the NTSC frame rate
while held, including when both the top-bar and TAS-window controls are visible.
Controller bindings are sampled even while the mouse-held advance button has UI
focus, so live rerecord input is not replaced with an empty controller state.

`Play read-only` disables timeline, metadata, marker, paste, and rerecord changes.
The TAS debug log reports mode transitions, frame progress, recording/rerecord
events, loads/saves, checkpoint activity, invalid data, ROM mismatches, and detected
desyncs.

### TAS Control View and external movie conversion

The separate TAS Control View opens FCEUX text `.fm2`, BizHawk `.bk2`, extracted
BizHawk `Input Log.txt`, and native `.tas` files without immediately changing the
running movie. It provides a virtualized two-controller input list, hexadecimal
masks, button names, frame jump, source metadata, rerecord count, and explicit
warnings for conversion differences.

After loading the matching NES ROM, choose power-on, reset, or current state and
use `Convert and open in TAS Editor`. The controller log becomes a native movie
and can then be edited, replayed, or saved normally. FM2/BK2 reset, power, disk,
and coin commands are displayed and preserved as warning markers but are not
executed. PAL timing, Four Score players 3/4, Zapper input, binary FM2 logs, and
foreign emulator save states are not silently approximated. See
[`docs/TAS_CONTROL_VIEW.md`](docs/TAS_CONTROL_VIEW.md).

### Hex editor and debugger

F2 opens the hex editor and pauses emulation. It exposes:

- CPU RAM, PPU nametable memory, palette RAM, and OAM as guarded writable views.
- PRG ROM as read-only.
- CHR as read-only for CHR-ROM games and writable for CHR-RAM games.
- Hexadecimal and ASCII columns, paging, byte selection/editing, and hexadecimal
  address jump.

All reads are side-effect-free snapshots. Writes use bounds-checked core methods;
invalid hexadecimal values, out-of-range offsets, and read-only writes are
rejected. The debugger reports CPU/PPU registers and timing, frame count, lag
count, and controller-read activity, with pause/resume and frame-step controls.

### Settings, input, audio, and video

Settings are grouped into General, Video, Audio, Input, Emulation, Paths, Save
States, TAS, and Debugging. Every category has a restore-default button. New
fields receive defaults when an older settings file is loaded without resetting
the existing known values. The Settings window can be closed with its explicit
button, its title-bar control, or Escape, and can be dragged outside the game area.

Safe changes apply immediately, including volume/mute, optional soft clipping,
integer scaling, FPS-target display, input mapping, speed, rewind limits, ROM
folder, slot count, and hex-page size. Native audio startup-buffer changes are
clearly marked as restart-required. Per-game overrides are available for volume,
mute, and speed without modifying the global defaults.

Controller settings default to hardware-style D-pad conflict handling: pressing
Left+Right or Up+Down together produces neutral direction input, matching the
stock NES controller's rocker. **Allow opposite D-pad directions** can be enabled
for specialized TAS/debug use. Existing TAS movie playback is not rewritten by
this live-controller preference.

Video settings include the default NTSC 2C02 approximation, the documented
RP2C03 RGB DAC palette used by PlayChoice-10, and imported custom palettes. The
palette applies immediately and remains a presentation preference across save
states, rewind, and TAS playback. See
[`docs/CUSTOM_PALETTES.md`](docs/CUSTOM_PALETTES.md) for supported formats.

The optional CRT display now includes a Royale-style advanced profile inspired
by CRT-Royale's documented rendering model. It adds gamma-correct luminance-
dependent beams, aperture-grille/slot/shadow masks, RGB convergence, faceplate
halation, glass diffusion, bloom, vignette, and barrel-curved edges. PVM and
consumer-TV presets are included, while the original lightweight profile remains
available for slower systems. The filter changes presentation only and does not
affect screenshots, save states, rewind, or TAS determinism. See
[`docs/CRT_FILTER.md`](docs/CRT_FILTER.md).

The separate **Flat CRT** profile keeps the advanced beam, phosphor, analog
softness, bloom, halation, diffusion, and convergence effects while disabling
curvature, vignette, and curved black screen borders.

Advanced CRT rows are processed in parallel, and normal-speed presentation uses
a small phase-lock tolerance to prevent timer jitter from producing a missed
frame followed by a two-frame catch-up. The FPS display uses a rolling two-second
measurement so short sampling-window quantization does not appear as false dips.

The Audio / Video window also has independent Pulse 1, Pulse 2, Triangle, Noise,
and DMC output gates. Muted channels still clock and DMC DMA/IRQ behavior remains
active, so channel isolation does not change emulated timing. Audio diagnostics
show the native device, sample rate, queued frames, underruns, and overflows.

## Persistent files and formats

On Windows, application data defaults to:

```text
%LOCALAPPDATA%\MyOwnNesEmulator\
  settings.json                 Global categorized settings
  per-game-settings.json        ROM-hash-keyed volume/mute/speed overrides
  library.json                  Opened games and recently played timestamps
  library-covers\               Copied custom game cover images
  palettes\                     Validated, normalized custom RGB palettes
  recent-roms.txt               Legacy list, read only for one-time migration
  states\<rom-hash>\slot-N.moss Versioned state + timestamp + RGB preview
  tas\<rom-hash>\               Default TAS import/export folder
```

`.moss` files have a versioned `MONESUI` wrapper around the versioned core
`MONESST` snapshot. Both store a 64-bit hash of the complete iNES file. `.tas`
movies use the emulator's readable `TAS_FORMAT 1` text format with a full ROM
SHA-256, emulator version, NTSC region, start type, rerecord count, optional author
and description, optional base64 embedded starting state, markers, checkpoint state
hashes, and one `frame|player1|player2` hexadecimal input line per frame. JSON
settings use `version: 1` and Serde defaults for forward additions. Files are
written through a temporary file and renamed to reduce partial-write risk.

Example:

```text
TAS_FORMAT 1
EMULATOR MyOwnNesEmulator
EMULATOR_VERSION 0.1.0
ROM_SHA256 0123456789abcdef...
REGION NTSC
START_TYPE POWER_ON
RERECORDS 12
PLAYERS 2

[INPUT]
0|00|00
1|80|00
2|80|01
```

The complete field and section specification is in
[`docs/TAS_FORMAT.md`](docs/TAS_FORMAT.md).

Battery-backed PRG RAM remains a `.sav` beside the ROM. Screenshots go into a
`screenshots` directory beside the ROM. Save states are not battery saves and are
never substituted for them.

## Project structure

```text
crates/
  nes-core/                 Platform-independent deterministic console
    src/
      apu/                  Channels, frame sequencer, mixer, filters, resampler
      bus.rs                CPU map, CPU/APU/PPU clocks, OAM and DMC DMA
      cartridge/            iNES parser, mapper interface, Mapper 0
      controller.rs         NES serial controllers
      cpu.rs                2A03/6502 CPU
      emulator.rs           Console facade, snapshots, debug memory APIs
      ppu.rs                2C02 timing, memory, and RGB frame generation
  nes-audio-native/         Native miniaudio backend and PCM ring buffer
  nes-cli/                  Headless frame runner
  nes-ui/                   Library, settings, states, TAS, hex/debug, A/V UI
```

CPU instruction cycles clock the APU once and PPU three times. Front-end frame
scheduling only decides how much emulated work to request; it does not act as an
emulation clock. The APU generates samples from the NTSC CPU clock, uses nonlinear
pulse/TND mixing and NES-style filters, and the native ring buffer handles device
synchronization independently of Unity or UI frame timing.

## Current limitations

- Only NTSC iNES 1.0 Mapper 0 is supported. PAL and Dendy are intentionally not
  mixed into the NTSC timing path.
- Only keyboard controller mapping is exposed; gamepad/device binding is not yet
  implemented.
- Library scans the selected folder's top level, not subdirectories, and ROM
  titles are derived from file names rather than an external metadata database.
- TAS checkpoints and embedded reset/save-state starts are intentionally
  uncompressed, so long sessions trade disk or memory for simple deterministic
  inspection. The editor has one active branch rather than a branch tree.
- Rewind uses periodic full snapshots, so longer buffers trade memory for history.
- The hex editor changes live machine memory but does not provide undo or ROM
  patch export.
- The PPU is not yet dot-perfect for every sprite evaluation/overflow and fetch
  pipeline quirk; unofficial CPU opcodes and mapper expansion remain future core
  accuracy work.

## Verification

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release --workspace
```

Core tests include save/load round trips, incompatible-state rejection, guarded
memory editing, and deterministic replay of an identical controller sequence from
the same starting snapshot, in addition to CPU/PPU/APU timing tests.

## Third-party notice

`nes-audio-native` vendors miniaudio 0.11.25 for native device access. Its license
is at `crates/nes-audio-native/native/LICENSE-miniaudio`. Selected APU timing and
filter behavior was adapted from the permissively licensed TetaNES project; the
attribution and MIT license are in `THIRD_PARTY_NOTICES.md`.

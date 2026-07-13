# My Own NES Emulator

A from-scratch NES emulation core in Rust. The first milestone
loads iNES 1.0 ROMs and runs Mapper 0 (NROM-128/NROM-256) cartridges through a
6502 CPU, CPU bus, timed PPU, controller ports, and five-channel APU synthesis.

This repository intentionally separates the emulation model from any desktop UI.
That keeps tests deterministic and lets a future front end pause, rewind, record
TAS input, change speed, or render through a different graphics API without
putting host-specific behavior into the virtual console.

## Play it

You need a legally obtained/homebrew `.nes` ROM.

```powershell
cargo test
cargo run --release -p nes-ui
```

Running without a path opens the native ROM browser. You can also pass a ROM
directly:

```powershell
cargo run --release -p nes-ui -- path\to\game.nes
```

On Windows, double-click `Play NES.bat` for the same ROM-browser experience. The
release executable is built at `target\release\nes-ui.exe`.

The separate CLI remains useful as a headless development harness:

```powershell
cargo run -p nes-cli -- path\to\game.nes --frames 120 --screenshot frame.png
```

### Controls

| Key | Action |
|---|---|
| Arrow keys | D-pad |
| Z / X | A / B |
| Enter / Shift | Start / Select |
| Space | Pause or resume |
| N | Advance one frame while paused |
| R | Console reset |
| Ctrl+R | Restart/reload cartridge |
| P | Power off or on |
| Tab (hold) | Fast-forward at 4x |
| + / - / 0 | Increase, decrease, or restore 1x speed |
| F11 | Fullscreen/windowed |
| F12 | Save a PNG under the ROM's `screenshots` folder |
| Ctrl+O | Open another ROM |
| Ctrl+1 through Ctrl+9 | Open a recent ROM |
| Escape | Exit and flush battery-backed RAM |

The window title displays the current ROM, frame number, run state, and speed.
Battery-backed saves use a `.sav` beside the ROM and are loaded automatically.
The persistent tab bar opens separate windows for Game, Save States,
Rewind/Speed, TAS, Input, Audio/Video, Library, and Debugger features. Features
whose core systems are not implemented yet are visible but disabled with an
explanation of the dependency they need.

## Project structure

```text
crates/
  nes-core/                 # Pure deterministic emulation; no OS or UI code
    src/
      cpu.rs                # 2A03/6502 registers, instructions, interrupts
      ppu.rs                # 2C02 registers, VRAM, timing, RGB frame output
      apu/
        mod.rs              # 2A03 APU registers, frame sequencer, IRQs, DMC DMA
        channels.rs         # Pulse, triangle, noise, and DMC state machines
        mixer.rs            # Nonlinear mixer, sampling, and DC blocking
      bus.rs                # CPU address decoding and component clocks
      controller.rs         # NES serial controller protocol
      cartridge/
        ines.rs             # iNES 1.0 parser
        mapper.rs           # Mapper interface
        nrom.rs             # Mapper 0 and cartridge PRG/CHR RAM
      emulator.rs           # Public console facade and frame/instruction stepping
  nes-cli/                  # Headless ROM runner and smoke-test front end
  nes-audio-native/         # Native miniaudio/WASAPI output and PCM ring buffer
  nes-ui/                   # Tabbed UI, controls, browser, audio/video presentation
```

Host-specific systems remain outside `nes-core`:

```text
crates/
  nes-audio-native/         # Native output backend; no Rust audio framework
  nes-video/                # Scaling, shaders, fullscreen presentation
  nes-ui/                   # Current playable frontend; debugger UI grows here
  nes-tools/                # TAS movie format, disassembler, trace comparison
```

Inside `nes-core`, add `snapshot`, `rewind`, and `tas` modules only after every
mutable emulated field has an explicit serializable state representation.

## How components communicate

The CPU never reaches directly into a PPU, controller, APU, or mapper. Every CPU
read/write goes through `Bus`, which owns the NES address map:

```text
Front end input ──> Controller state
                         │
CPU <──read/write──> CPU Bus <──> 2 KiB RAM
                         ├──────> PPU registers ──> Cartridge CHR through mapper
                         ├──────> APU registers
                         ├──────> Controller serial ports
                         └──────> Cartridge PRG/RAM through mapper

CPU instruction cycles ──> Bus clocks APU 1x and PPU 3x
APU DMC request ───────────> Bus reads cartridge PRG and stalls CPU 4 cycles
APU/frame IRQ ─────────────> CPU IRQ at the next instruction boundary
PPU vertical blank ───────> NMI pending ───────> CPU on its next boundary
PPU frame complete ───────> RGB frame ─────────> Front end
```

The cartridge owns a mapper behind a small interface. CPU and PPU accesses are
offered to that mapper, so adding MMC1/MMC3 does not alter CPU instruction code.
Later, mapper clock hooks can carry PPU address transitions and CPU clocks for
IRQ counters and expansion audio.

The front end should submit controller state for a specific frame and consume a
completed frame/audio samples. It should never use keyboard events as emulated
time. That rule makes save states, rewind, frame advance, and TAS playback
deterministic.

## Recommended implementation order

### 1. Executable NROM baseline (current milestone)

- Strict iNES 1.0 loading, 16/32 KiB PRG, CHR ROM/CHR RAM, mirroring, PRG RAM.
- All official 6502 instructions/addressing modes, interrupts, page-cross timing,
  stack behavior, indirect-JMP hardware bug, and OAM DMA stalls.
- PPU CPU-facing registers, buffered reads, VRAM/palette mirroring, VBlank/NMI,
  3:1 PPU/CPU timing, loopy scroll transfers, and background/sprite rendering.
- Two pulse channels, triangle, noise, DMC playback/DMA, envelopes,
  length/linear counters, sweep, nonlinear mixing, and 48 kHz sampling.
- Native miniaudio/WASAPI output with a lock-free PCM ring buffer, startup
  prebuffering, underrun recovery, and an optional reference-mastering stage.
- Controller strobing/serial reads and a headless frame runner.

Before calling this milestone accurate, run `nestest`, `blargg` CPU tests, PPU
VBlank/NMI tests, sprite tests, and timing ROMs. The current renderer is a useful
bring-up renderer, not yet a dot-accurate 2C02: sprite evaluation/overflow quirks,
  PPU fetch-pipeline side effects, most unofficial CPU opcodes, and more exhaustive
  APU timing/IRQ validation remain.

### 2. Finish NROM compatibility

1. Replace pixel sampling with the real PPU background fetch/shift-register and
   sprite-evaluation pipelines, including hardware overflow quirks.
2. Validate DMC DMA conflicts, frame-counter edge cases, and channel timing against
   dedicated APU conformance ROMs and hardware recordings.
3. Implement commonly used unofficial 6502 opcodes and bus-conflict/open-bus
   behavior required by tests.
4. Add trace logging and automated reference-ROM tests before building UI polish.

### 3. Deterministic state and playback

1. Define versioned state structs for CPU, PPU internal latches/pipeline, APU,
   RAM, mapper registers/RAM, controllers, DMA, and master timing.
2. Save/load slots are serialization around that single full-machine snapshot.
3. Rewind stores periodic full snapshots plus compressed intermediate snapshots
   in a bounded ring; audio is discarded/regenerated after restore.
4. TAS movies store ROM hash, initial state/power-on, and controller bits per
   frame. Editing truncates/re-simulates downstream state; rerecord count belongs
   in movie metadata.

One snapshot implementation must power manual saves, rewind, TAS seeking, and
debugger checkpoints. Separate versions of “state” inevitably drift.

### 4. Mapper expansion

Suggested order by value and complexity:

1. Mapper 2 (UxROM), 3 (CNROM), 7 (AxROM)
2. Mapper 1 (MMC1)
3. Mapper 4 (MMC3, including scanline IRQ behavior)
4. Region variants and additional boards based on desired game coverage

Extend the mapper interface with explicit reset, snapshot, CPU clock, and PPU A12
edge callbacks as needed. Do not make a mapper know about the UI or whole bus.

### 5. Desktop front end and tools

- A command/state controller owns stopped/running/paused modes. Restart reloads
  the ROM; reset asserts console reset; power-off stops emulated clocks.
- The scheduler targets audio buffer fullness, with 1x/variable speed,
  fast-forward (usually mute/drop presentation frames), and one-frame stepping.
- Video owns aspect-correct integer scaling, window/fullscreen, screenshots, and
  optional filters. Audio owns device selection, volume, latency, and resampling.
- Input mappings translate host keyboard/gamepad controls into NES button state.
- UI persistence owns recent ROMs, settings, paths, slot metadata, and battery
  `.sav` files; none belong in the core.
- Debugger reads a side-effect-free bus view and controls instruction-boundary
  breakpoints, watchpoints, stepping, registers, memory, and disassembly.

## Architectural rules that keep this extensible

- Emulated time comes only from component clocks, never wall-clock time.
- Core output is frames and timestamped audio; core input is controller state and
  explicit commands.
- Mapper-specific registers stay inside the mapper.
- Reads used by a debugger must be separate from reads with hardware side effects.
- State files include a format version, ROM hash, region, mapper ID, and checksum.
- Battery RAM is persisted atomically by the front end and is not a save state.
- Add accuracy tests before optimizations; optimize measured hot paths afterward.

## Third-party audio backend

`nes-audio-native` vendors miniaudio 0.11.25 for native device access only. The
NES APU implementation remains original project code. Miniaudio is available
under its public-domain or MIT No Attribution license; the vendored license is
at `crates/nes-audio-native/native/LICENSE-miniaudio`.

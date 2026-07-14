# Test ROM runner

CrabNes includes a headless runner for NES test ROMs that implement the common
blargg `$6000` status protocol. Test ROM binaries are not bundled with CrabNes;
obtain freely distributed tests from their authors or the
[NESdev emulator-test catalogue](https://www.nesdev.org/wiki/Emulator_tests).

Run one ROM:

```powershell
cargo run -p nes-cli --bin crabnes-test-rom --locked -- path\to\test.nes
```

Run every `.nes` file below one or more directories:

```powershell
cargo run -p nes-cli --bin crabnes-test-rom --locked -- tests\cpu tests\ppu
```

The default budget is 30 emulated NTSC CPU seconds. It and the number of reset
requests a test may issue can be changed explicitly:

```powershell
cargo run -p nes-cli --bin crabnes-test-rom --locked -- `
  --max-cycles 100000000 --max-resets 8 tests
```

The process exits successfully only when every discovered ROM passes. Output is
one line per ROM:

```text
PASS tests\01-basics.nes (537467 cycles, 180895 instructions, 0 resets) — Passed
FAIL tests\example.nes code $03 (12345 cycles, 4567 instructions, 0 resets) — detail
TIMEOUT tests\example.nes after 53693190 cycles and 18000000 instructions
ERROR tests\example.nes — unsupported opcode $0B at $03A0
```

## Protocol

The runner waits for the signature `DE B0 61` at CPU addresses `$6001-$6003`.
After observing the running value `$80` at `$6000`, it interprets these values:

- `$00`: passed;
- `$01-$7F` or `$82-$FF`: failed with that code;
- `$80`: still running;
- `$81`: reset requested.

A null-terminated diagnostic string may begin at `$6004`. Reads performed by
the harness are side-effect free.

## Timing status

CPU memory accesses are scheduled individually. Every opcode fetch, operand
fetch, data read, data write, stack access, vector read, interrupt, and reset
slot advances the APU by one CPU clock and the NTSC PPU by three dots. Remaining
documented instruction slots are explicit idle cycles, and OAM/DMC DMA stalls
are serviced before the next CPU bus slot.

This is the scheduling foundation for cycle accuracy, not the end of the
accuracy work. Some addressing modes still use an idle slot where hardware
would perform a specific dummy read or write. Stable unofficial CPU opcodes and
several PPU fetch/evaluation quirks also remain to be implemented. The test-ROM
runner makes those gaps visible and prevents later fixes from regressing already
passing behavior.

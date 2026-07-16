# CrabNes TAS format

`TAS_FORMAT 1` is a UTF-8 text format owned by this emulator. It is not FM2 and
does not copy FCEUX's movie serialization. Blank lines and lines beginning with
`#` are ignored.

## Required metadata

```text
TAS_FORMAT 1
EMULATOR CrabNes
EMULATOR_VERSION 0.1.0
ROM_SHA256 <64 lowercase or uppercase hex characters>
REGION NTSC
START_TYPE POWER_ON | RESET | SAVE_STATE
RERECORDS <unsigned decimal integer>
PLAYERS 2
```

`AUTHOR` and `DESCRIPTION` are optional. Backslash, newline, and carriage return
are encoded as `\\`, `\n`, and `\r`. Unknown metadata keys and unknown named
sections are ignored so optional additions do not invalidate version 1 readers.

The loader rejects unsupported versions, non-NTSC movies, unknown start types,
invalid masks, missing required fields, nonsequential input rows, malformed
embedded states, and ROM SHA-256 mismatches. A differing emulator version is a
warning rather than an error.

## Starting state

`POWER_ON` reconstructs a fresh console from the matching ROM. `RESET` movies
created by the UI embed the exact post-reset machine state so cartridge RAM and
timing are reproducible. `SAVE_STATE` requires an embedded full-machine state.

Embedded states are base64 text in an optional `[STATE]` section:

```text
[STATE]
TU9ORVNTVAEAAAA...
```

The payload is the same versioned core snapshot used by normal save states and
contains CPU, PPU, APU, RAM, mapper, controllers, DMA, interrupts, timing, and
power state. No screenshots, audio recordings, or video frames are stored.

## Markers and state hashes

Bookmarks use decimal frame numbers:

```text
[MARKERS]
120|Before first jump
845|Boss room
```

Optional state hashes describe the machine state immediately before the keyed
frame. They allow playback to detect divergence at automatic checkpoint
boundaries:

```text
[CHECKSUMS]
0|<64 hex characters>
300|<64 hex characters>
600|<64 hex characters>
```

On a mismatch, CrabNes independently replays the preceding checkpoint interval.
If replay and live execution agree, the old checksum is treated as stale
metadata and refreshed. If replay instead reproduces the movie's expected
checksum, playback restores that verified state and continues automatically.
Playback pauses only when neither result can be verified, so recovery never
silently accepts an unexplained state.

Runtime checkpoint states are deliberately not serialized. They are rebuilt in
memory while playing or seeking, keeping the movie inspectable and preventing it
from accumulating many full snapshots.

## Frame input

`[INPUT]` is required. Rows must be sequential and use decimal frame numbers and
two hexadecimal controller bytes:

```text
[INPUT]
0|00|00
1|81|00
2|81|40
3|00|40
```

The columns are `frame|player1|player2`. Each byte is the complete controller
state for that emulated frame:

```text
Bit 0 = A
Bit 1 = B
Bit 2 = Select
Bit 3 = Start
Bit 4 = Up
Bit 5 = Down
Bit 6 = Left
Bit 7 = Right
```

A checked bit on consecutive rows is a held button, not repeated host keyboard
events. Inputs are applied to both emulated controller ports immediately before
the console runs the corresponding frame.

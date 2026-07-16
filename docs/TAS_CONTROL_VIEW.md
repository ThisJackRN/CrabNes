# TAS Control View

TAS Control View is an inspection and conversion tool for controller-input movie
files. Opening a file does not start playback or alter the current native movie.

## Supported inputs

- FCEUX FM2 version 3 with a text input log (`.fm2`).
- BizHawk BK2 archives containing `Input Log.txt` (`.bk2`, deflated or stored).
- Extracted BizHawk/NesHawk `[Input]` logs (`.txt` or `.log`).
- This emulator's native `TAS_FORMAT 1` (`.tas`, matching ROM required).

The FM2 controller order `RLDUTSBA` and the BizHawk NES order `UDLRSsBA` are
converted to the native mask order A, B, Select, Start, Up, Down, Left, Right.
Player 1 and Player 2 are retained.

## Conversion workflow

1. Open **TAS Control** from the main toolbar.
2. Select **Open movie** and inspect the decoded frame list and warnings.
3. Load the exact NES ROM and revision used to make the source movie.
4. Select power-on, reset, current state, or the embedded FCEUX state as the
   native starting condition.
5. Select **Convert and open in TAS Editor**.
6. Replay and verify synchronization, then save with the native TAS editor.

Conversion copies controller frames, author/description where available, and the
rerecord count. External reset/power/disk/coin commands become markers labelled
`not applied`, so a lossy conversion is visible in both the Control View and the
native editor.

## Accuracy and safety limits

- Text FM2 movies with an embedded chunked FCS state can seed the native
  deterministic state for MMC3 (mapper 4). CPU, PPU, APU registers, RAM,
  controllers, and mapper state are translated and then stored as a native
  CrabNes state. Other FCEUX mapper states are rejected explicitly.
- FM2 ROM MD5 is checked against the loaded ROM's PRG+CHR payload before an
  embedded state is imported. BK2 ROM identity metadata is not currently
  cross-checked, so BK2 users must select the same dump and revision.
- BizHawk savestates remain emulator-specific and cannot seed the native state.
- PAL inputs can be inspected, but the current emulator is NTSC-only and will not
  remain synchronized with a PAL movie.
- Binary FM2, FCM, Mesen movie files, Zapper/paddle data, FDS behavior, and Four
  Score players 3/4 are not converted.
- Files and decompressed archive entries are limited to 128 MiB. Malformed text,
  missing BK2 input logs, non-NES BK2 platforms, and invalid controller rows are
  rejected without altering emulator state.

FM2 parsing follows the public [FCEUX FM2 format
documentation](https://fceux.com/web/help/fm2.html).

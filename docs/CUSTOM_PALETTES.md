# Custom color palettes

Open **Settings > Video** or **Audio / Video**, then choose **Import palette…**.
The emulator validates the file, copies a normalized 192-byte version into its
application-data folder, selects **Custom imported palette**, and applies it
immediately. Moving or deleting the original file does not break the imported
copy.

Supported formats:

- Binary `.pal`: exactly 192 bytes, with 64 consecutive RGB888 triples.
- Extended binary `.pal`: 1536 bytes (eight 192-byte emphasis tables). The base
  table is imported; the additional emphasis tables are currently ignored.
- UTF-8 text: exactly 64 colors, one per line. A color may be `#RRGGBB`,
  `RRGGBB`, `0xRRGGBB`, or three comma/space-separated decimal RGB values.
  Components with `0x` prefixes are hexadecimal. Blank lines and `;` comments
  are accepted.
- JASC-PAL text with a declared color count of 64.

Imported files are stored under:

```text
%LOCALAPPDATA%\CrabNes\palettes\custom-<hash>.pal
```

The **RGB 2C03 / PlayChoice-10** built-in uses the RP2C03/RP2C05 three-bit DAC
codes documented by NESdev, expanded to RGB888. It changes the output-color
lookup only. It does not switch PPU timing, register quirks, or game/system type,
so it is safe to use as a visual preference with an ordinary NES ROM.

The **RGB 2C04-0004 (Vs. System)** built-in uses the color PROM from the RP2C04-0004
PPU revision found in Vs. Super Mario Bros. arcade boards. It is the most
accurate choice for that specific game; other Nintendo Vs. System titles used
different RP2C04-000x PROMs with different color assignments, so it is an
approximation for them.

## Nintendo Vs. System (mapper 99)

Vs. System arcade boards used an RGB PPU rather than the composite NTSC 2C02 in
home consoles, and almost universally an RP2C04-000x chip whose color index
wiring is scrambled per revision as an anti-piracy measure. Running Vs. System
tile data through the wrong chip family's decoding does not just look
different — it renders the wrong colors outright (for example, a PlayChoice-10
decoding of Vs. Super Mario Bros. renders a neon green background and magenta
bricks instead of blue sky and brown bricks).

A mapper 99 ROM always renders with **RGB 2C04-0004 (Vs. System)** — the chip
Vs. Super Mario Bros. was authored for, and the closest built-in family for
other Vs. System titles even when their board used a different RP2C04
revision — unless that specific game has an explicit palette override. This is
a per-game fallback, not a one-time default: the global palette setting used by
every other ROM (Settings > Video) is never the implicit answer for a Vs. game,
including after a per-game override is turned back off.

Pick a different palette for a specific Vs. game in **Per-game overrides >
Override palette** — shown in both the Settings window and the quick Audio /
Video window, expanded by default — choosing PlayChoice-10, NTSC, RP2C04-0004
explicitly, or a custom imported palette. It sticks until you turn the
override off again. If you change the palette dropdown above it while a Vs.
game is loaded, a "Use the palette above for just this game" button appears as
a shortcut that sets the override to match.

Output palette selection is deliberately excluded from core save-state and TAS
machine data. Loading a state or rewinding therefore preserves the user's active
palette and cannot change deterministic emulation results.

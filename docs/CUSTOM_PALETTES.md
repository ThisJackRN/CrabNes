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

Output palette selection is deliberately excluded from core save-state and TAS
machine data. Loading a state or rewinding therefore preserves the user's active
palette and cannot change deterministic emulation results.

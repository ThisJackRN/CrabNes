# CRT display filter

Enable **CRT display (3× phosphor raster)** under **Settings > Video** or in the
**Audio / Video** window. The filter applies immediately, including while the
emulator is paused.

The renderer expands every 256×240 NES frame into a 768×720 raster. Each source
pixel receives a stable 3×3 beam/phosphor cell, which keeps the RGB triads and
scanline pattern aligned at the common 3× integer display size. It includes:

- A three-line electron-beam brightness profile.
- An RGB phosphor/shadow-mask pattern.
- Horizontal analog-video bandwidth softening.
- Adjustable highlight bloom.
- Edge vignette and mild barrel curvature.
- Black curved overscan edges instead of stretching the image corners.

## Royale-style advanced profile

The advanced profile is an original renderer inspired by CRT-Royale's documented
techniques; it does not copy or embed the Libretro shader. In addition to the
base effects, it provides:

- Gamma-correct processing in linear light.
- Luminance-dependent Gaussian-style beam width.
- Selectable aperture-grille/PVM, slot-mask/consumer-TV, and shadow-mask layouts.
- Independent faceplate halation and glass-diffusion controls.
- Adjustable red/blue convergence offsets.
- PVM and consumer television presets.

The **Lightweight CRT** profile retains the original single color pass for
systems where the advanced profile is too expensive. CRT-Royale itself is a
multi-pass Libretro shader and cannot be loaded directly by this emulator's egui
renderer.

## Flat CRT profile

**Flat CRT (no screen geometry)** uses the same gamma-correct dynamic beams,
selectable phosphor masks, analog softness, bloom, halation, glass diffusion,
and convergence controls as the advanced profile. It deliberately forces a
rectangular image with no barrel distortion, vignette, corner crop, or curved
black borders. Use **Flat CRT / PVM preset** for a balanced starting point.

Every component can be adjusted separately or disabled by turning off the main
CRT checkbox. The selected values are stored in `settings.json` and older
settings files receive safe defaults automatically.

This is a presentation filter. Raw NES frame pixels remain unchanged, so core
save states, rewind snapshots, TAS checksums, library artwork, and standard PNG
screenshots are unaffected.

The advanced and flat profiles process scanline rows in parallel and initialize
their worker pool before gameplay, avoiding both sustained single-core frame
pressure and a first-use hitch. If a very low-core system still cannot maintain
full speed, select **Lightweight CRT**.

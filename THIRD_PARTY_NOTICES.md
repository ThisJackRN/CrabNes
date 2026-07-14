# Third-party notices

This file distinguishes code or assets included in the repository from projects
and documentation used only as design or interoperability references. A project
listed as a reference is not thereby incorporated into this project's source or
license.

## FCEUX TAS and FM2 reference (no incorporated code)

The native TAS workflow was informed by FCEUX and its TAS Editor, including the
general ideas of frame-by-frame controller editing, rerecord tracking, markers,
read-only playback, and seeking with cached emulator states. FM2 import follows
FCEUX's public [FM2 format documentation](https://fceux.com/web/help/fm2.html).

The TAS and FM2 implementations in `crates/nes-ui/src/tas.rs` and
`crates/nes-ui/src/tas_control.rs` were independently written in Rust. No FCEUX
source code, artwork, or binary assets are copied, translated, linked, or
distributed in this repository.

FCEUX is developed by the FCEUX contributors and is distributed under the GNU
General Public License version 2. Its upstream source is available at
<https://github.com/TASEmulators/fceux>. This acknowledgement credits the design
reference and file-format documentation; it does not apply FCEUX's GPL to this
independent implementation.

## BizHawk movie-format reference (no incorporated code)

BK2 and extracted `Input Log.txt` import interoperates with controller logs
produced by BizHawk/NesHawk. The parser was independently written from the
text/archive format and does not include BizHawk source code, cores, or assets.
Thanks to the BizHawk contributors for the format and tooling. Upstream source:
<https://github.com/TASEmulators/BizHawk>.

## NESdev hardware documentation (no incorporated code)

NES hardware behavior and the RP2C03 palette values are based in part on the
community-maintained technical documentation at <https://www.nesdev.org/wiki/>.
Those factual hardware references informed the implementation; no NESdev wiki
source code or site content is distributed here.

## miniaudio (vendored code)

`crates/nes-audio-native/native/vendor/miniaudio.h` vendors miniaudio 0.11.25 by
David Reid. Its public-domain/MIT dual-license text is preserved at
`crates/nes-audio-native/native/vendor/LICENSE-miniaudio`. Upstream source:
<https://github.com/mackron/miniaudio>.

## rcheevos (vendored code)

`crates/nes-achievements-native/native/rcheevos` vendors rcheevos 12.3.0 from
RetroAchievements. It provides ROM hashing and the achievement client/runtime;
the emulator provides the network transport, memory interface, and desktop UI.
The upstream source is <https://github.com/RetroAchievements/rcheevos>. rcheevos
is distributed under the MIT License, preserved at
`crates/nes-achievements-native/native/rcheevos/LICENSE`.

## TetaNES APU reference

Portions of the APU frame-counter timing, length-counter collision handling,
DMC start timing, and audio-filter cutoff selection in `crates/nes-core/src/apu`
were adapted from TetaNES (`tetanes-core/src/apu`) at commit
`7e42001f7745eaae02706cbe827adf57203af51a`.
Upstream source: <https://github.com/lukexor/tetanes>.

Copyright (c) 2021 Luke Petherbridge

MIT License

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

TetaNES is also offered upstream under Apache-2.0. This project uses the
MIT-licensed grant above for the adapted portions.

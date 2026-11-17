# Dadaspec Style (`web/styles/dadaspec/`)

Real-time 3D acoustic spectrum adaptation of **[dadabots/dadaspec](https://github.com/dada-bots/dadaspec)** (`983e8df`).

## Visual & Acoustic Grammar

Unlike static rainbow heatmaps (which map dB amplitude or fixed X position to a fixed spectrum), Dadaspec separates **per-bin luminance** from **per-frame tri-band spectral balance**:

1. **Per-Band Normalized Chromaticity (`< 200 Hz`, `200–1500 Hz`, `>= 1500 Hz`)**
   - `Renderer` computes linear band power across Dadaspec's three perceptual zones on every incoming row and maintains a 6-level `8 × 4096` `RGBA8` ring pyramid (`chromaTex`, 128 KiB VRAM) aligned 1:1 with the shared `R8` history pyramid — requiring **one texture read and zero fragment loops** in `surface.frag`.
   - Solo energy in a single zone holds pure primary saturation (`Low -> Red`, `Mid -> Green`, `High -> Blue`) regardless of whether the passage is loud (`-2 dBFS`) or quiet (`-70 dBFS`).
   - Multi-band broadband energy (`R + G + B`) converges additively to **crisp chalk white**, while partial band combinations reveal structural arrangement shifts across time (`Low + Mid` amber-gold, `Mid + High` seafoam-teal, `Low + High` cobalt-violet).
   - In 3D and Front views, mid-slope flanks retain subtle prismatic acoustic-zone dispersion so sustained broadband chords preserve sculptural depth beneath chalk-white crests.

2. **Perceptual Contrast & Stop-Start Seams**
   - A strict `#000000` silence gate and low-dB velvet toe suppress room noise and FFT sidelobes, transitioning into Dadaspec's `0.62` perceptual gamma across the active musical dynamic range.
   - `chromaTex.a` stores a row-rate-invariant (`50–480 rows/s`) `40 ms` onset-flux envelope that highlights sharp rhythmic attacks while steep cliff occlusion and quiet silent-floor lines preserve pitch-black stop-start gaps.

3. **Instrument Scale & Modes**
   - Front-rail caliper ticks, subtle floor rails, front-view zone dividers, and piano-strip markers register the exact **`200 Hz`** (`band 43.35`) and **`1.5 kHz`** (`band 78.23`) crossover boundaries.
   - One fixed colourway (`RGB Tri-Band`) and four reinterpreted terrain modes (`Lit Surface`, `Ridgeline`, `Mesh`, `Solid`) work across all three camera presets, four frame sizes, and `60–480 rows/s × 1–8 bands/semitone`.

## Attribution and licence

This style is named after, and takes its colouring idea from, **[dadaspec](https://github.com/dada-bots/dadaspec)** by dadabots (revision `983e8dfa1ff8d2356a78171689d1eec6dac5d1aa`, MIT licence). It is not affiliated with or endorsed by dadabots.

What comes from dadaspec:

- The idea itself: each moment in time gets one colour from the balance of three frequency bands, and each frequency bin sets only the brightness.
- The three bands and their colours: below 200 Hz red, 200 to 1500 Hz green, 1500 Hz and above blue.
- Normalising each band on its own, and the perceptual gamma of about 0.6 on the dB scale.

What is different here:

- dadaspec renders a still thumbnail of a whole file (a stereo mel spectrogram, left channel above, right channel mirrored below) and normalises each band over the whole file. This style runs live, so each band instead follows its own recent level (fast attack, slow release) and is mapped over the 36 dB below that.
- The picture is a 3D terrain with three camera views, not a flat image.
- The onset highlight, the silence gate, the 200 Hz and 1.5 kHz markers on the scale, and the exact primaries (`#ff2b1c`, `#1ae668`, `#3382ff`, adding up to a cool white) are additions.

No source code from `dadaspec.py` is copied. The JavaScript, GLSL, Rust and CSS for this style were written for this project. The dadaspec licence is reproduced in full because the design follows it closely:

```text
MIT License

Copyright (c) 2026 dadabots

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

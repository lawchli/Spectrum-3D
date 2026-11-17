// Shared by every vertex shader so the surface, grid, front spectrum and guides use one mapping.
// GLSL ES 3.00: the same source compiles in WebGL2 and in Android OpenGL ES 3.0.

uniform sampler2D u_history;  // R8 history pyramid: LEVELS blocks of dataBands() columns side by side, each a ring of u_rows rows;
                              // block l holds the power mean of 2^l data rows. 0.5 dB steps from -120 dBFS (the format the Spar recorder sends)
uniform float u_rows;         // ring height of every level
uniform float u_sub;          // data bands per semitone: 1 (127 bands, the phone's format), 2, 4 or 8
uniform float u_rowScale;     // data rows per 1/60 s: 1 at 60 rows a second, 8 at 480. Spacing in rows times this stays the same in time
uniform float u_newest;       // absolute index of the newest data row, modulo u_rows * 2^(LEVELS-1)
uniform float u_phase;        // 0..1 progress toward the next row, so scrolling stays smooth between data frames
uniform float u_spanRows;     // data rows from the front edge to the end of the tail
uniform vec2 u_dbRange;       // floor and ceiling in dBFS
uniform vec3 u_size;          // world size of the framed part: x frequency, y level, z time
uniform float u_tail;         // the whole waterfall is u_tail times the framed depth; the rest runs off into the fog
uniform float u_melAxis;      // frequency axis: 0 the same width for every semitone, 1 the mel scale, blended in between

const float BANDS = 127.0;    // semitone positions C0 16.35 Hz .. F#10 23.7 kHz; "band" below is in semitones
const float LEVELS = 6.0;     // same as renderer.js

float dataBands() { return (BANDS - 1.0) * u_sub + 1.0; }

// Texture x of semitone position `band` in level l.
float historyU(float band, float l) {
    float n = dataBands();
    return (l * n + clamp(band * u_sub, 0.0, n - 1.0) + 0.5) / (LEVELS * n);
}

float dbFromTexel(float v) { return v * 127.5 - 120.0; }

float levelFromDb(float db) {
    return clamp((db - u_dbRange.x) / (u_dbRange.y - u_dbRange.x), 0.0, 1.0);
}

// dB of `band` in the level-l block that starts at absolute data row r (a multiple of 2^l):
// the power mean of data rows r .. r + 2^l - 1. Read at the texel centre, so it never blends
// two blocks along time.
float blockDb(float band, float r, float l) {
    float block = mod(floor(r / exp2(l) + 0.5), u_rows);
    vec2 uv = vec2(historyU(band, l), (block + 0.5) / u_rows);
    return dbFromTexel(texture(u_history, uv).r);
}

// dB of `band` at a fractional absolute data row, from level l, blending neighbouring blocks
// along time through texture filtering. For per-pixel reads in the top view.
float rowDb(float band, float row, float l) {
    float span = exp2(l);
    float block = (row - 0.5 * (span - 1.0)) / span;
    vec2 uv = vec2(historyU(band, l), (mod(block, u_rows) + 0.5) / u_rows);
    return dbFromTexel(textureLod(u_history, uv, 0.0).r);
}

// Where semitone position `band` sits across the width, 0..1 (bands.js bandToU). The mel end
// maps ln(1 + hz / 700) between the axis's own end frequencies, C0 16.3516 Hz and F#10 23.68 kHz.
float bandToU(float band) {
    float lin = band / (BANDS - 1.0);
    float mel = log(1.0 + 16.3516 * exp2(band / 12.0) / 700.0);
    return mix(lin, (mel - 0.0230908) / (3.5504234 - 0.0230908), u_melAxis);
}

vec3 worldPos(float u, float level, float t) {
    return vec3((u - 0.5) * u_size.x, level * u_size.y, u_size.z * (0.5 - t * u_tail));
}

// Half a step of screen-space noise for an 8-bit colour: added to dark gradients (the fog, the
// fill under the live curve) so they do not show as bands.
float dither(vec2 fragCoord) {
    return (fract(52.9829189 * fract(dot(fragCoord, vec2(0.06711056, 0.00583715)))) - 0.5) / 255.0;
}

// Inverse of worldPos for z: where along the whole length (0 front, 1 end of the tail) z lies.
float tFromZ(float z) { return (0.5 - z / u_size.z) / u_tail; }

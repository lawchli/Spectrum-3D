#version 300 es
precision highp float;
#include "common.glsl"

uniform sampler2D u_palette;     // 256 x 1 OKLab energy ramp
uniform vec3 u_background;
uniform vec3 u_keyLight;
uniform vec3 u_rimLight;
uniform float u_ambient;
uniform float u_exposure;      // overall brightness, 1 = as designed
uniform float u_surfaceMix;      // 0 = black hidden-line occluder, 1 = shaded terrain
uniform float u_sliceStrength;   // scrolling waterfall slices moving with audio history
uniform float u_sliceEvery;      // data rows per slice at 60 rows a second (u_rowScale keeps it in time)
uniform float u_ribStrength;     // longitudinal harmonic ribs
uniform float u_ribEvery;        // semitone bands per rib
uniform float u_contourStrength; // 12 dB topographic elevation rings
uniform float u_octaveStrength;  // C-octave structural lines
uniform float u_lineFloor;       // minimum palette position for lines on silent floor
uniform float u_registerTint;    // subtle warmth in bass / cool clarity in highs on active audio
uniform float u_topness;         // 0 -> 1 in top-down spectrogram view
uniform float u_frontness;       // 0 -> 1 in flat front 2D analyzer view
uniform float u_fogStart;
uniform float u_fogPower;

in float v_level;
in float v_t;
in float v_band;
in vec3 v_normal;
out vec4 outColor;

void main() {
    // Semitone coordinate (0 = C0 16.35 Hz .. 126 = F#10 23.7 kHz), from the mesh: exact at its
    // columns (two a semitone or one per data band) and straight in between.
    float band = v_band;

    // Absolute data row under this pixel (the mesh rows sit on whole rows, see terrain.vert).
    float age = max(v_t * u_spanRows - u_phase, 0.0);
    float absRow = u_newest - age;
    float rowFw = fwidth(absRow);

    // Looking straight down, read the history per pixel like a spectrogram image instead of
    // the mesh's interpolated level: blended between neighbouring data bands and rows by the
    // texture filter, so it shows no cells, from the coarsest level the screen still resolves.
    // Newer blocks of coarse levels may still be filling; far down the tail the finer rings
    // no longer hold the row.
    float ringLod = min(ceil(log2(max((age + 2.0) / (u_rows - 2.0), 1.0))), LEVELS - 1.0);
    float topLod = clamp(floor(min(log2(max(rowFw, 1e-4)), log2(max(age, 1.0)) - 2.0)), ringLod, LEVELS - 1.0);
    float topLevel = levelFromDb(rowDb(band, absRow, topLod));
    float lvl = mix(v_level, topLevel, smoothstep(0.5, 0.95, u_topness));

    // Keep ADC and room floor pitch black; concentrate colour contrast across musical range.
    float energy = smoothstep(0.025, 0.995, lvl);
    vec3 pal = texture(u_palette, vec2(energy, 0.5)).rgb;

    // Subtle timbral register cue on active energy only (never colours the silent floor):
    // warm bronze grounding below ~180 Hz, crisp platinum air above ~3.5 kHz.
    float activeMask = smoothstep(0.15, 0.46, lvl);
    float lowReg = 1.0 - smoothstep(14.0, 44.0, band);
    float highReg = smoothstep(78.0, 114.0, band);
    vec3 regTint = mix(vec3(1.0), vec3(1.09, 0.97, 0.87), lowReg * u_registerTint);
    regTint = mix(regTint, vec3(0.93, 0.99, 1.08), highReg * u_registerTint);
    pal *= mix(vec3(1.0), regTint, activeMask);

    // Dual-light studio sculpting in 3D; relaxes to flat accurate energy in Top spectrogram view.
    vec3 n = normalize(v_normal);
    float keyDiff = max(dot(n, normalize(u_keyLight)), 0.0);
    float rimDiff = max(dot(n, normalize(u_rimLight)), 0.0);
    float steep = clamp(1.0 - n.y, 0.0, 1.0);
    float shade3d = u_ambient + (1.0 - u_ambient) * (0.76 * keyDiff + 0.28 * rimDiff)
                    + 0.18 * steep * activeMask;
    float shade = mix(shade3d, 1.03, u_topness * 0.90);

    // Even hidden-line slice styles reveal energy fill when looking straight down as a spectrogram.
    float effectiveSurface = mix(u_surfaceMix, max(u_surfaceMix, 0.94), u_topness);
    vec3 surfColor = mix(u_background, pal * shade, effectiveSurface);

    // Soft self-luminous crown on hot peaks (-18 .. 0 dBFS).
    float hotCrest = smoothstep(0.75, 0.97, lvl);
    surfColor += pal * (0.15 * hotCrest * effectiveSurface);

    // Analytical scrolling waterfall slices locked to absolute history rows so sustained synth drones visibly flow.
    float sliceCoord = absRow / (u_sliceEvery * u_rowScale);
    float sliceFw = fwidth(sliceCoord);
    float sliceVis = (1.0 - smoothstep(0.24, 0.44, sliceFw))
                     * (1.0 - 0.85 * u_topness)
                     * (1.0 - 0.88 * u_frontness);
    float sliceDist = abs(fract(sliceCoord - 0.5) - 0.5) / max(sliceFw, 1e-4);
    float sliceMask = (1.0 - smoothstep(0.0, 1.18, sliceDist)) * sliceVis * u_sliceStrength;

    // Analytical longitudinal harmonic ribs.
    float ribCoord = band / u_ribEvery;
    float ribFw = fwidth(ribCoord);
    float ribVis = (1.0 - smoothstep(0.24, 0.44, ribFw)) * (1.0 - 0.72 * u_topness);
    float ribDist = abs(fract(ribCoord - 0.5) - 0.5) / max(ribFw, 1e-4);
    float ribMask = (1.0 - smoothstep(0.0, 1.14, ribDist)) * ribVis * u_ribStrength;

    // Exact C-octave structural lines (every 12 semitones from C0).
    float octCoord = band / 12.0;
    float octFw = fwidth(octCoord);
    float octVis = 1.0 - smoothstep(0.20, 0.42, octFw);
    float octDist = abs(fract(octCoord - 0.5) - 0.5) / max(octFw, 1e-4);
    float innerBand = smoothstep(1.2, 3.0, band) * (1.0 - smoothstep(123.0, 125.2, band));
    float octMask = (1.0 - smoothstep(0.0, 1.15, octDist)) * octVis * innerBand * u_octaveStrength;

    // 12 dB topographic elevation rings on active 3D slopes.
    float db = mix(u_dbRange.x, u_dbRange.y, lvl);
    float dbCoord = db / 12.0;
    float dbFw = fwidth(dbCoord);
    float contourSlope = smoothstep(0.0018, 0.009, dbFw) * (1.0 - smoothstep(0.20, 0.38, dbFw));
    float contourDist = abs(fract(dbCoord - 0.5) - 0.5) / max(dbFw, 1e-4);
    float contourMask = (1.0 - smoothstep(0.0, 1.10, contourDist)) * contourSlope
                        * smoothstep(0.16, 0.30, lvl) * (1.0 - u_frontness) * u_contourStrength;

    float lineLevel = clamp(max(u_lineFloor, energy + 0.16 * activeMask), 0.0, 1.0);
    vec3 linePal = texture(u_palette, vec2(lineLevel, 0.5)).rgb * mix(vec3(1.0), regTint, activeMask);
    vec3 sliceCol = linePal * (0.68 + 0.50 * activeMask);
    float gridMask = clamp(max(sliceMask, ribMask * (0.30 + 0.70 * activeMask)), 0.0, 1.0);
    vec3 color = mix(surfColor, max(surfColor, sliceCol), gridMask);

    vec3 octCol = mix(vec3(0.09, 0.11, 0.14), pal * 1.26 + vec3(0.05), activeMask);
    color = mix(color, max(color, octCol), octMask * (0.26 + 0.54 * activeMask));

    vec3 contourCol = pal * 1.22 + vec3(0.045);
    color = mix(color, max(color, contourCol), contourMask);

    // In the flat front view the waterfall goes dark: only the live curve is drawn there.
    color *= 1.0 - u_frontness;

    color *= u_exposure;

    // C^1-smooth endless tail extinction: reaches exact background at v_t = 0.96 with zero slope (no Mach band).
    // From the top the framed part stays at full strength; the fade starts where it ends.
    float fogStart = mix(u_fogStart, 1.0 / u_tail, u_topness);
    float fogX = clamp((v_t - fogStart) / (0.96 - fogStart), 0.0, 1.0);
    float fogVis = 1.0 - pow(fogX, u_fogPower);
    fogVis *= fogVis;

    // Gentle nocturnal desaturation far down the tail before vanishing into #000000.
    float farCool = smoothstep(0.26, 0.86, fogX);
    float lum = dot(color, vec3(0.2126, 0.7152, 0.0722));
    vec3 cooled = mix(color, vec3(lum) * vec3(0.80, 0.89, 1.03), farCool * 0.50);

    outColor = vec4(max(mix(u_background, cooled, fogVis) + dither(gl_FragCoord.xy) * fogVis, 0.0), 1.0);
}

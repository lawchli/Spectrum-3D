#version 300 es
precision highp float;
#include "common.glsl"

// Dadaspec 3D waterfall surface shader (v2 after self-review).
// Preserves pure primary saturation on solo tones all the way to 0 dBFS ("one with only low end
// goes red however loud it is"), reserves additive chalk-white saturation strictly for multi-band
// broadband energy while retaining 3D slope dispersion on chord flanks, applies a velvet low-dB toe
// beneath Dadaspec's 0.62 musical gamma, and keeps silent stop-start gaps pitch-dark even in Ridgeline.

uniform sampler2D u_chroma;        // 8 x u_rows RGBA8: columns 0..5 hold levels 0..5 (RGB = band norms, A = onset flux)
uniform vec3 u_background;
uniform vec3 u_keyLight;
uniform vec3 u_rimLight;
uniform float u_ambient;
uniform float u_exposure;
uniform vec3 u_lowColor;
uniform vec3 u_midColor;
uniform vec3 u_highColor;
uniform vec3 u_whiteColor;
uniform float u_chromaSat;
uniform float u_dispersion;
uniform float u_surfaceMix;
uniform float u_sliceStrength;
uniform float u_sliceEvery;
uniform float u_ribStrength;
uniform float u_ribEvery;
uniform float u_channelEtch;
uniform float u_seamBoost;
uniform float u_contourStrength;
uniform float u_octaveStrength;
uniform float u_crossoverStrength;
uniform float u_lineFloor;
uniform float u_topness;
uniform float u_frontness;
uniform float u_fogStart;
uniform float u_fogPower;

in float v_level;
in float v_t;
in float v_band;
in vec3 v_normal;
out vec4 outColor;

vec4 readChroma(float row, float l) {
    float span = exp2(l);
    float block = (row - 0.5 * (span - 1.0)) / span;
    vec2 uv = vec2((l + 0.5) * 0.125, (mod(block, u_rows) + 0.5) / u_rows);
    return textureLod(u_chroma, uv, 0.0);
}

void main() {
    // 1. Unconditional screen-space derivatives at top of main() for Adreno 540 portability.
    float band = v_band;  // semitone position from the mesh (terrain.vert)
    float bandFw = fwidth(band);

    float subBand = band * u_sub;
    float subFw = fwidth(subBand);

    float age = max(v_t * u_spanRows - u_phase, 0.0);
    float absRow = u_newest - age;
    float rowFw = fwidth(absRow);

    float sliceCoord = absRow / max(u_sliceEvery * u_rowScale, 0.25);
    float sliceFw = fwidth(sliceCoord);
    float ribCoord = band / max(u_ribEvery, 0.125);
    float ribFw = fwidth(ribCoord);
    float octCoord = band / 12.0;
    float octFw = fwidth(octCoord);
    float dbCoord = mix(u_dbRange.x, u_dbRange.y, v_level) / 12.0;
    float dbFw = fwidth(dbCoord);

    // 2. Per-pixel history readout in Top view, blended between bands and rows by the texture
    // filter; locked mesh height in 3D/Front view.
    float ringLod = min(ceil(log2(max((age + 2.0) / (u_rows - 2.0), 1.0))), LEVELS - 1.0);
    float topLod = clamp(floor(min(log2(max(rowFw, 1e-4)), log2(max(age, 1.0)) - 2.0)), ringLod, LEVELS - 1.0);
    float topLevel = levelFromDb(rowDb(band, absRow, topLod));
    float lvl = mix(v_level, topLevel, smoothstep(0.48, 0.94, u_topness));

    float chromaLod = clamp(floor(min(max(log2(max(rowFw, 1.0)), 0.0), log2(max(age, 1.0)) - 2.0)), ringLod, LEVELS - 1.0);
    vec4 frameChroma = readChroma(absRow, chromaLod);

    // 3. Perceptual dB contrast curve: strict #000000 silence gate + velvet low-dB toe (suppressing
    //    FFT sidelobes) + Dadaspec's 0.62 gamma across the active musical span.
    float sigGate = smoothstep(0.025, 0.085, lvl);
    float velvetToe = smoothstep(0.025, 0.30, lvl);
    float musSpan = clamp((lvl - 0.14) / 0.86, 0.0, 1.0);
    float dadaBright = sigGate * mix(velvetToe * 0.24, 0.22 + 0.78 * pow(musSpan, 0.62), smoothstep(0.14, 0.34, lvl));
    float litMask = smoothstep(0.10, 0.38, lvl);

    // 4. Dual-scale tri-band chromatic synthesis:
    //    - Solo tones (triBalance == 0) stay vivid primary R/G/B even at 0 dBFS;
    //    - Broadband frames fuse additively to chalk white at ridge crests while retaining
    //      sculptural acoustic-zone dispersion on 3D mid-slopes so sustained chords keep depth.
    float wLow = 1.0 - smoothstep(39.85, 46.85, band);
    float wHigh = smoothstep(74.73, 81.73, band);
    float wMid = clamp(1.0 - wLow - wHigh, 0.0, 1.0);
    vec3 regWeights = vec3(wLow, wMid, wHigh);

    vec3 fw = frameChroma.rgb;
    float fwPeak = max(fw.r, max(fw.g, fw.b));
    vec3 frameWeights = fwPeak > 0.035 ? (fw / fwPeak) : regWeights;

    float triBalance = min(frameWeights.r, min(frameWeights.g, frameWeights.b));
    float multiBand = clamp((frameWeights.r + frameWeights.g + frameWeights.b - 1.0) * 0.5, 0.0, 1.0);

    // Keep stronger local register relief on 3D slopes (lvl < 0.75), converging on loud broadband crests.
    float crestFuse = smoothstep(0.58, 0.90, lvl) * triBalance;
    float dispMix = u_dispersion * mix(1.0, 0.45, u_topness) * (1.0 - 0.68 * crestFuse);
    // Only blend in local register weight when multi-band energy is present (never tint a solo tone's sidelobes wrong).
    vec3 mixW = mix(frameWeights, regWeights, dispMix * smoothstep(0.08, 0.45, multiBand));

    vec3 rawRgb = u_lowColor * mixW.x + u_midColor * mixW.y + u_highColor * mixW.z;
    float rawPeak = max(rawRgb.r, max(rawRgb.g, rawRgb.b));
    vec3 baseHue = rawPeak > 1e-4 ? (rawRgb / rawPeak) : u_whiteColor;

    // Strictly multi-band additive white: a solo tone never washes out to white!
    float broadWhite = smoothstep(0.46, 0.96, triBalance)
                       * mix(0.28 + 0.68 * smoothstep(0.52, 0.90, lvl), 0.56 + 0.40 * smoothstep(0.32, 0.85, lvl), u_topness);
    vec3 chromaHue = mix(baseHue, u_whiteColor, clamp(broadWhite, 0.0, 1.0));

    float hueLum = dot(chromaHue, vec3(0.2126, 0.7152, 0.0722));
    chromaHue = mix(vec3(hueLum) * u_whiteColor, chromaHue, u_chromaSat);

    vec3 dadaColor = chromaHue * dadaBright;

    // 5. Discrete mel/sub-band channel micro-grooves + rate-invariant onset seam highlight.
    float chanVis = 1.0 - smoothstep(0.20, 0.44, subFw);
    float chanEdge = abs(fract(subBand) - 0.5) / max(subFw, 1e-4);
    float chanGroove = (1.0 - smoothstep(0.0, 1.05, chanEdge)) * chanVis * u_channelEtch * mix(1.0, 1.18, u_topness);
    dadaColor *= 1.0 - chanGroove;

    float onset = frameChroma.a * u_seamBoost * litMask;
    dadaColor += mix(chromaHue, u_whiteColor, 0.45 * multiBand + 0.15) * (0.22 * onset * dadaBright);

    // 6. Studio 3D lighting + steep temporal seam occlusion for crisp stop-start cuts.
    vec3 n = normalize(v_normal);
    float keyDiff = max(dot(n, normalize(u_keyLight)), 0.0);
    float rimDiff = max(dot(n, normalize(u_rimLight)), 0.0);
    float steep = clamp(1.0 - n.y, 0.0, 1.0);
    float seamCliff = abs(n.z) * (1.0 - smoothstep(0.04, 0.22, lvl));
    float shade3d = (u_ambient + (1.0 - u_ambient) * (0.76 * keyDiff + 0.30 * rimDiff) + 0.22 * steep * litMask)
                    * (1.0 - 0.35 * seamCliff);
    float shade = mix(shade3d, 1.02, u_topness * 0.92);

    float effectiveSurface = mix(u_surfaceMix, max(u_surfaceMix, 0.96), u_topness);
    vec3 surfColor = mix(u_background, dadaColor * shade, effectiveSurface);

    // Specular ridge energy: stays pure primary on solo tones, chalk-white on multi-band peaks.
    float hotBloom = smoothstep(0.80, 0.98, lvl);
    surfColor += mix(chromaHue, u_whiteColor, 0.72 * multiBand) * (0.12 * hotBloom * effectiveSurface);

    // 7. Analytical temporal frame slices & harmonic ribs (neutral whisper on silent floor so stop-start gaps stay #000000).
    float sliceVis = (1.0 - smoothstep(0.24, 0.44, sliceFw))
                     * (1.0 - 0.82 * u_topness)
                     * (1.0 - 0.88 * u_frontness);
    float sliceDist = abs(fract(sliceCoord - 0.5) - 0.5) / max(sliceFw, 1e-4);
    float sliceMask = (1.0 - smoothstep(0.0, 1.16, sliceDist)) * sliceVis * u_sliceStrength;

    float ribVis = (1.0 - smoothstep(0.24, 0.44, ribFw)) * (1.0 - 0.74 * u_topness);
    float ribDist = abs(fract(ribCoord - 0.5) - 0.5) / max(ribFw, 1e-4);
    float ribMask = (1.0 - smoothstep(0.0, 1.14, ribDist)) * ribVis * u_ribStrength;

    vec3 silentLine = vec3(0.75, 0.82, 0.92) * u_lineFloor;
    vec3 activeLine = mix(chromaHue, u_whiteColor, 0.32 * multiBand) * clamp(dadaBright * 1.14 + 0.12, 0.0, 1.15);
    vec3 lineCol = mix(silentLine, activeLine, sigGate);
    float gridMask = clamp(max(sliceMask * (0.35 + 0.65 * sigGate), ribMask * (0.22 + 0.78 * litMask)), 0.0, 1.0);
    vec3 color = mix(surfColor, max(surfColor, lineCol), gridMask);

    // 8. Dadaspec 200 Hz (band 43.350) & 1500 Hz (band 78.233) acoustic crossover filaments.
    float xoverVis = 1.0 - smoothstep(0.30, 0.65, bandFw);
    float d200 = abs(band - 43.350) / max(bandFw, 1e-4);
    float d1500 = abs(band - 78.233) / max(bandFw, 1e-4);
    float m200 = (1.0 - smoothstep(0.0, 1.20, d200)) * xoverVis * u_crossoverStrength;
    float m1500 = (1.0 - smoothstep(0.0, 1.20, d1500)) * xoverVis * u_crossoverStrength;
    vec3 c200 = normalize(u_lowColor + u_midColor + vec3(1e-4));
    vec3 c1500 = normalize(u_midColor + u_highColor + vec3(1e-4));
    c200 = mix(vec3(0.08, 0.09, 0.11), mix(c200, u_whiteColor, 0.32) * 1.12, litMask);
    c1500 = mix(vec3(0.08, 0.09, 0.11), mix(c1500, u_whiteColor, 0.32) * 1.12, litMask);
    color = mix(color, max(color, c200), m200 * (0.18 + 0.62 * litMask));
    color = mix(color, max(color, c1500), m1500 * (0.18 + 0.62 * litMask));

    // Secondary C-octave reference hairlines.
    float octVis = 1.0 - smoothstep(0.20, 0.42, octFw);
    float octDist = abs(fract(octCoord - 0.5) - 0.5) / max(octFw, 1e-4);
    float innerBand = smoothstep(1.2, 3.0, band) * (1.0 - smoothstep(123.0, 125.2, band));
    float octMask = (1.0 - smoothstep(0.0, 1.12, octDist)) * octVis * innerBand * u_octaveStrength;
    vec3 octCol = mix(vec3(0.07, 0.08, 0.10), mix(chromaHue, u_whiteColor, 0.45 * multiBand + 0.20) * 1.08, litMask);
    color = mix(color, max(color, octCol), octMask * (0.18 + 0.50 * litMask));

    // 12 dB topographic elevation rings on active 3D slopes.
    float contourSlope = smoothstep(0.0018, 0.009, dbFw) * (1.0 - smoothstep(0.20, 0.38, dbFw));
    float contourDist = abs(fract(dbCoord - 0.5) - 0.5) / max(dbFw, 1e-4);
    float contourMask = (1.0 - smoothstep(0.0, 1.08, contourDist)) * contourSlope
                        * smoothstep(0.20, 0.36, lvl) * (1.0 - u_frontness) * (1.0 - u_topness) * u_contourStrength;
    vec3 contourCol = mix(chromaHue, u_whiteColor, 0.40 * multiBand + 0.20) * (dadaBright * 1.16 + 0.05);
    color = mix(color, max(color, contourCol), contourMask);

    color *= 1.0 - u_frontness;  // the flat front view shows only the live curve
    color *= u_exposure;

    // 9. C^1-smooth endless tail extinction to exact #000000 at v_t = 0.96.
    float fogStart = mix(u_fogStart, 1.0 / u_tail, u_topness);
    float fogX = clamp((v_t - fogStart) / (0.96 - fogStart), 0.0, 1.0);
    float fogVis = 1.0 - pow(fogX, u_fogPower);
    fogVis *= fogVis;

    float farFade = smoothstep(0.34, 0.90, fogX);
    float lum = dot(color, vec3(0.2126, 0.7152, 0.0722));
    vec3 tailTinted = mix(color, vec3(lum) * vec3(0.86, 0.92, 1.02), farFade * 0.40);

    outColor = vec4(max(mix(u_background, tailTinted, fogVis) + dither(gl_FragCoord.xy) * fogVis, 0.0), 1.0);
}

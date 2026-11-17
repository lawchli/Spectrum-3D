#version 300 es
precision highp float;
#include "common.glsl"

// Dadaspec live leading-edge spectrum shader (v2 after self-review).
// Matches surface.frag's exact multi-band chroma physics so curtain and newest waterfall row join
// seamlessly, while keeping a crisp chalk-white leading trace for instant pitch/headroom legibility.

uniform float u_dpr;
uniform vec4 u_liveChroma;
uniform vec3 u_lowColor;
uniform vec3 u_midColor;
uniform vec3 u_highColor;
uniform vec3 u_whiteColor;
uniform float u_chromaSat;
uniform float u_dispersion;
uniform vec3 u_traceColor;
uniform vec3 u_peakColor;
uniform vec3 u_headroomColor;
uniform vec3 u_maxColor;
uniform float u_exposure;
uniform float u_mode;          // 0 = curtain + anti-aliased trace, 1 = peak hold, 2 = core 1px edge, 3 = max hold
uniform float u_frontness;
uniform float u_topness;

in float v_level;
in float v_top;
in float v_band;
out vec4 outColor;

void main() {
    // Evaluate all screen-space derivatives unconditionally at top of main() (Adreno 540 portability).
    float band = v_band;
    float grad = length(vec2(dFdx(v_top), dFdy(v_top)));
    float bandFw = fwidth(band);

    float wLow = 1.0 - smoothstep(39.85, 46.85, band);
    float wHigh = smoothstep(74.73, 81.73, band);
    float wMid = clamp(1.0 - wLow - wHigh, 0.0, 1.0);
    vec3 regWeights = vec3(wLow, wMid, wHigh);

    vec3 fw = u_liveChroma.rgb;
    float fwPeak = max(fw.r, max(fw.g, fw.b));
    vec3 frameWeights = fwPeak > 0.035 ? (fw / fwPeak) : regWeights;
    float triBalance = min(frameWeights.r, min(frameWeights.g, frameWeights.b));
    float multiBand = clamp((frameWeights.r + frameWeights.g + frameWeights.b - 1.0) * 0.5, 0.0, 1.0);

    float crestFuse = smoothstep(0.58, 0.90, v_level) * triBalance;
    float dispMix = mix(u_dispersion, min(0.54, u_dispersion + 0.16), u_frontness) * (1.0 - 0.65 * crestFuse);
    vec3 mixW = mix(frameWeights, regWeights, dispMix * smoothstep(0.08, 0.45, multiBand));

    vec3 rawRgb = u_lowColor * mixW.x + u_midColor * mixW.y + u_highColor * mixW.z;
    float rawPeak = max(rawRgb.r, max(rawRgb.g, rawRgb.b));
    vec3 baseHue = rawPeak > 1e-4 ? (rawRgb / rawPeak) : u_whiteColor;

    // Strictly multi-band white convergence on curtain fill so solo tones keep deep primary chroma.
    float broadWhite = smoothstep(0.46, 0.96, triBalance) * (0.30 + 0.65 * smoothstep(0.50, 0.90, v_level));
    float hot = smoothstep(0.76, 0.95, v_level);
    vec3 chromaHue = mix(baseHue, u_whiteColor, clamp(broadWhite, 0.0, 1.0));
    float hueLum = dot(chromaHue, vec3(0.2126, 0.7152, 0.0722));
    chromaHue = mix(vec3(hueLum) * u_whiteColor, chromaHue, u_chromaSat);

    float sigLevel = smoothstep(0.025, 0.16, v_level);
    float velvetToe = smoothstep(0.025, 0.30, v_level);
    float musSpan = clamp((v_level - 0.14) / 0.86, 0.0, 1.0);
    float dadaBright = sigLevel * mix(velvetToe * 0.24, 0.22 + 0.78 * pow(musSpan, 0.62), smoothstep(0.14, 0.34, v_level));
    vec3 curtainHue = chromaHue * (0.42 + 0.68 * dadaBright) * u_exposure;

    if (u_mode < 0.5) {
        float dpDist = ((1.0 - v_top) / max(grad, 1e-5)) / max(u_dpr, 1.0);
        float basePx = (v_top / max(grad, 1e-5)) / max(u_dpr, 1.0);

        float xoverVis = 1.0 - smoothstep(0.30, 0.65, bandFw);
        float xover = max(
            1.0 - smoothstep(0.0, 1.25, abs(band - 43.350) / max(bandFw, 1e-4)),
            1.0 - smoothstep(0.0, 1.25, abs(band - 78.233) / max(bandFw, 1e-4))
        ) * xoverVis;

        // As in the shared front.frag: per-column fade in 3D, height-on-screen fade in the flat front view.
        float bodyAlpha = mix(pow(v_top, 1.55) * (0.052 + 0.24 * smoothstep(0.08, 0.80, v_level)),
                              0.06 + 0.22 * pow(v_level * v_top, 1.3), u_frontness);
        float baseStrip = (1.0 - smoothstep(0.6, 2.2, basePx)) * smoothstep(0.08, 0.32, v_level) * 0.34;
        float halo = exp(-dpDist / 5.5) * smoothstep(0.08, 0.42, v_level) * mix(0.18, 0.30, u_frontness);

        float stroke = (1.0 - smoothstep(0.55, 1.72, dpDist)) * smoothstep(0.01, 0.055, v_level);
        vec3 quietTrace = u_traceColor * 0.48;
        vec3 activeTrace = mix(chromaHue * 0.46 + u_traceColor * 0.62, u_traceColor, 0.48 + 0.52 * triBalance);
        vec3 crestTrace = mix(activeTrace, mix(u_headroomColor, vec3(1.0), 0.60), hot);
        vec3 traceCol = mix(quietTrace, crestTrace, sigLevel);

        float totalFill = bodyAlpha + baseStrip;
        vec3 fillCol = mix(curtainHue, mix(curtainHue, u_whiteColor, 0.5), xover * 0.35);
        float alpha = clamp(max(totalFill, stroke * 0.96), 0.0, 1.0);
        vec3 premult = fillCol * (totalFill + halo) * (1.0 - stroke * 0.86) + traceCol * stroke * 0.96;
        outColor = vec4(max(premult + dither(gl_FragCoord.xy) * smoothstep(0.0, 0.02, alpha), 0.0), alpha);
    } else if (u_mode < 1.5) {
        float vis = smoothstep(0.04, 0.15, v_level) * (1.0 - u_topness) * (1.0 - u_frontness);
        vec3 regHue = mix(u_peakColor, chromaHue * 0.85 + vec3(0.22), 0.48 * u_chromaSat);
        vec3 col = mix(regHue, u_headroomColor, smoothstep(0.74, 0.91, v_level));
        float a = vis * mix(0.38, 0.54, u_frontness) * (1.0 + 0.32 * hot);
        outColor = vec4(col * a, a);
    } else if (u_mode > 2.5) {
        float vis = smoothstep(0.04, 0.13, v_level) * (1.0 - u_topness);
        float a = vis * mix(0.50, 0.66, u_frontness);
        outColor = vec4(u_maxColor * a, a);
    } else {
        vec3 liveEdge = mix(chromaHue * 0.32 + u_traceColor * 0.74, mix(u_traceColor, u_headroomColor, 0.35), hot);
        vec3 baseCol = mix(u_traceColor * 0.34, liveEdge, sigLevel);
        float a = mix(0.22, 0.92, sigLevel) * mix(1.0, 0.58, u_topness * (1.0 - sigLevel));
        outColor = vec4(baseCol * a, a);
    }
}

#version 300 es
precision highp float;
#include "common.glsl"

uniform sampler2D u_palette;
uniform float u_dpr;
uniform vec3 u_traceColor;
uniform vec3 u_peakColor;
uniform vec3 u_headroomColor;
uniform vec3 u_maxColor;
uniform float u_exposure;
uniform float u_mode;       // 0 = fill + anti-aliased trace, 1 = peak hold (3D only), 2 = core leading edge, 3 = max hold
uniform float u_frontness;  // 0 -> 1 in flat front 2D view
uniform float u_topness;    // 0 -> 1 in top-down spectrogram view

in float v_level;
in float v_top;
out vec4 outColor;

void main() {
    // Evaluate screen-space derivatives at top level for strict GLSL ES 3.00 driver portability (Adreno 540).
    float grad = length(vec2(dFdx(v_top), dFdy(v_top)));

    float energy = smoothstep(0.02, 0.99, v_level);
    vec3 pal = texture(u_palette, vec2(clamp(energy * 1.05 + 0.06, 0.0, 1.0), 0.5)).rgb * u_exposure;
    float hot = smoothstep(0.76, 0.95, v_level);
    float activeLevel = smoothstep(0.025, 0.18, v_level);

    if (u_mode < 0.5) {
        // Fill + implicit-distance anti-aliased live spectrum trace (premultiplied alpha).
        // The fill is one plain gradient from the curve down to the floor, as on a parametric
        // EQ's analyser: no stripes, and dithered so the dark end does not band.
        float dpDist = ((1.0 - v_top) / max(grad, 1e-5)) / max(u_dpr, 1.0);

        // In 3D each column fades from its own top, brighter where it is louder. In the flat
        // front view the fill depends only on the height on screen (v_level * v_top is the
        // level at this pixel) and has one colour, so neighbouring bands of different height
        // do not show as vertical streaks.
        float bodyAlpha = mix(pow(v_top, 1.80) * (0.045 + 0.18 * smoothstep(0.08, 0.80, v_level)),
                              0.05 + 0.20 * pow(v_level * v_top, 1.3), u_frontness);
        vec3 fillCol = mix(pal, texture(u_palette, vec2(0.60, 0.5)).rgb * u_exposure, 0.85 * u_frontness);
        float halo = exp(-dpDist / 6.0) * smoothstep(0.06, 0.40, v_level) * mix(0.18, 0.28, u_frontness);

        float stroke = (1.0 - smoothstep(0.55, 1.70, dpDist)) * smoothstep(0.008, 0.05, v_level);
        vec3 quietTrace = u_traceColor * 0.52;
        vec3 midTrace = mix(u_traceColor, pal * 1.15 + vec3(0.18), 0.32);
        vec3 crestTrace = mix(midTrace, mix(u_headroomColor, vec3(1.0, 0.97, 0.90), 0.55), hot);
        vec3 traceCol = mix(quietTrace, crestTrace, activeLevel);

        float alpha = clamp(max(bodyAlpha, stroke * 0.96), 0.0, 1.0);
        vec3 premult = fillCol * (bodyAlpha + halo) * (1.0 - stroke * 0.85) + traceCol * stroke * 0.96;
        outColor = vec4(max(premult + dither(gl_FragCoord.xy) * smoothstep(0.0, 0.02, alpha), 0.0), alpha);
    } else if (u_mode < 1.5) {
        // Peak-hold memory filament: invisible on silent floor, warms to tungsten-amber on hot peaks.
        // Gone in the flat front view, where only the live curve and max hold are drawn.
        float vis = smoothstep(0.035, 0.14, v_level) * (1.0 - u_topness) * (1.0 - u_frontness);
        vec3 col = mix(u_peakColor, u_headroomColor, smoothstep(0.72, 0.90, v_level));
        float a = vis * mix(0.34, 0.48, u_frontness) * (1.0 + 0.35 * hot);
        outColor = vec4(col * a, a);
    } else if (u_mode > 2.5) {
        // Max hold: the loudest level since the last clear, a thin solid line dimmer than the live trace.
        float vis = smoothstep(0.035, 0.12, v_level) * (1.0 - u_topness);
        float a = vis * mix(0.50, 0.66, u_frontness);
        outColor = vec4(u_maxColor * a, a);
    } else {
        // Core 1px leading edge (reinforces trace crispness, quiet baseline at silence, and '0s' edge in Top view).
        vec3 baseCol = mix(u_traceColor * 0.36, mix(u_traceColor, u_headroomColor, hot * 0.45), activeLevel);
        float a = mix(0.22, 0.90, activeLevel) * mix(1.0, 0.55, u_topness * (1.0 - activeLevel));
        outColor = vec4(baseCol * a, a);
    }
}

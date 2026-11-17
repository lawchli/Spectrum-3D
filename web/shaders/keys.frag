#version 300 es
precision highp float;

uniform vec3 u_whiteKey;
uniform vec3 u_blackKey;
uniform vec3 u_litKey;
uniform vec3 u_hoverKey;
uniform vec3 u_octaveEdge;
uniform float u_alpha;

flat in float v_key;
flat in float v_lit;
flat in float v_hover;
in float v_across;
out vec4 outColor;

void main() {
    // Derivatives first, outside any branch (Adreno 540 portability).
    float px = fwidth(v_across);

    // Band 0 is C0, so the pitch class is the band index mod 12. Offset by half a key and
    // floored, so a GPU that divides by multiplying with 1/12 cannot land C on 12.
    float pc = floor(mod(v_key + 0.5, 12.0));
    float black = float(pc == 1.0 || pc == 3.0 || pc == 6.0 || pc == 8.0 || pc == 10.0);
    vec3 col = mix(u_whiteKey, u_blackKey, black);
    col = mix(col, u_hoverKey, v_hover * 0.55);
    col = mix(col, u_litKey, clamp(v_lit, 0.0, 1.0) * 0.85);

    // A one-pixel seam between keys while keys are wide enough to show it, and a brighter
    // left edge on every C so the octaves read at a glance.
    float wide = 1.0 - smoothstep(0.22, 0.45, px);
    float edge = min(v_across, 1.0 - v_across) / max(px, 1e-4);
    float seam = (1.0 - smoothstep(0.4, 1.2, edge)) * wide;
    col *= 1.0 - 0.6 * seam;
    float octave = float(pc == 0.0) * (1.0 - smoothstep(0.6, 1.6, v_across / max(px, 1e-4)));
    col = mix(col, u_octaveEdge, octave);

    outColor = vec4(col * u_alpha, u_alpha);
}

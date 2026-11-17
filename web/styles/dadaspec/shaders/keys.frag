#version 300 es
precision highp float;

// Dadaspec piano strip shader.
// Idle keys stay neutral obsidian/graphite so the bottom edge stays calm; detected peaks light keys
// in their acoustic zone's Dadaspec primary (<200 Hz low, 200-1500 Hz mid, >1500 Hz high), blooming
// additively toward chalk white on the strongest peak.

uniform vec3 u_whiteKey;
uniform vec3 u_blackKey;
uniform vec3 u_litKey;
uniform vec3 u_hoverKey;
uniform vec3 u_octaveEdge;
uniform vec3 u_lowColor;
uniform vec3 u_midColor;
uniform vec3 u_highColor;
uniform float u_chromaSat;
uniform float u_alpha;

flat in float v_key;
flat in float v_lit;
flat in float v_hover;
in float v_across;
out vec4 outColor;

void main() {
    // Unconditional derivative at top of main() (Adreno 540 portability).
    float px = fwidth(v_across);

    float pc = floor(mod(v_key + 0.5, 12.0));
    float isBlack = float(pc == 1.0 || pc == 3.0 || pc == 6.0 || pc == 8.0 || pc == 10.0);
    vec3 col = mix(u_whiteKey, u_blackKey, isBlack);

    // Register primary for this semitone key (200 Hz = key 43.35, 1500 Hz = key 78.23).
    float wLow = 1.0 - smoothstep(41.0, 46.0, v_key);
    float wHigh = smoothstep(76.0, 81.0, v_key);
    float wMid = clamp(1.0 - wLow - wHigh, 0.0, 1.0);
    vec3 zoneRgb = u_lowColor * wLow + u_midColor * wMid + u_highColor * wHigh;
    float zonePeak = max(zoneRgb.r, max(zoneRgb.g, zoneRgb.b));
    vec3 zoneHue = zonePeak > 1e-4 ? (zoneRgb / zonePeak) : u_litKey;
    zoneHue = mix(u_litKey, zoneHue, u_chromaSat);

    col = mix(col, mix(u_hoverKey, zoneHue, 0.35), v_hover * 0.60);
    float litAmt = clamp(v_lit, 0.0, 1.0);
    // Main peak (litAmt -> 1.0) shifts toward crisp chalk white, secondary peaks (~0.6) hold vivid zone chroma.
    vec3 peakCol = mix(zoneHue, u_litKey, smoothstep(0.62, 1.0, litAmt) * 0.52);
    col = mix(col, peakCol, litAmt * 0.90);

    float wide = 1.0 - smoothstep(0.22, 0.45, px);
    float edge = min(v_across, 1.0 - v_across) / max(px, 1e-4);
    float seam = (1.0 - smoothstep(0.4, 1.2, edge)) * wide;
    col *= 1.0 - 0.62 * seam;

    // Brighter left seam on every C, plus subtle zone-divider accent near G#3 (key 44) and F#6 (key 78).
    float leftEdge = 1.0 - smoothstep(0.6, 1.6, v_across / max(px, 1e-4));
    float octave = float(pc == 0.0) * leftEdge;
    col = mix(col, u_octaveEdge, octave);

    float xoverKey = float(abs(v_key - 44.0) < 0.4 || abs(v_key - 78.0) < 0.4) * leftEdge;
    col = mix(col, mix(u_octaveEdge, zoneHue, 0.55), xoverKey * 0.75);

    outColor = vec4(col * u_alpha, u_alpha);
}

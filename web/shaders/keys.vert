#version 300 es
precision highp float;
#include "common.glsl"

// Piano strip under the frequency axis: one key per semitone band. It hangs a fixed number
// of pixels below the front edge in screen space, so it reads the same in the front, top
// and 3D views.
layout(location = 0) in float a_band;  // key edge, in bands (k - 0.5 or k + 0.5)
layout(location = 1) in float a_side;  // 0 at the top of the strip, 1 at the bottom
layout(location = 2) in float a_key;   // band index of the key
layout(location = 3) in float a_lit;   // 0 idle, up to 1 while a labelled peak sits on the key

uniform mat4 u_viewProj;
uniform vec2 u_viewport;
uniform float u_keyGapPx;
uniform float u_keyHeightPx;
uniform float u_hoverBand;  // band under the cursor, or -10 when none

flat out float v_key;
flat out float v_lit;
flat out float v_hover;
out float v_across;  // 0 at the left edge of the key, 1 at the right

void main() {
    v_key = a_key;
    v_lit = a_lit;
    v_hover = 1.0 - step(0.5, abs(u_hoverBand - a_key));
    v_across = step(a_key, a_band);
    float band = clamp(a_band, 0.0, BANDS - 1.0);
    vec4 p = u_viewProj * vec4(worldPos(bandToU(band), 0.0, 0.0), 1.0);
    p.y -= (u_keyGapPx + a_side * u_keyHeightPx) * 2.0 / u_viewport.y * p.w;
    gl_Position = p;
}

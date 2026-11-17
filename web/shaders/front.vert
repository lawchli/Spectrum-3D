#version 300 es
precision highp float;
#include "common.glsl"

// The live spectrum on the front edge of the waterfall. One vertex per band (line),
// or two per band for the curtain below it (a_top = 1 at the curve, 0 on the floor).
layout(location = 0) in float a_band;
layout(location = 1) in float a_db;
layout(location = 2) in float a_top;

uniform mat4 u_viewProj;
uniform float u_lift;
uniform float u_tilt;  // dB per octave about 1 kHz added to the drawn level; 0 draws true dBFS

out float v_level;
out float v_top;
out float v_band;

void main() {
    // 1 kHz is semitone 71.2131 from C0. Bands with no data (-120) stay on the floor.
    float level = levelFromDb(a_db + u_tilt * (a_band - 71.2131) / 12.0 * step(-119.0, a_db));
    v_level = level;
    v_top = a_top;
    v_band = a_band;
    gl_Position = u_viewProj * vec4(worldPos(bandToU(a_band), level * a_top, 0.0) + vec3(0.0, u_lift * a_top, 0.0), 1.0);
}

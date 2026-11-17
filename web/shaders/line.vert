#version 300 es
precision highp float;
#include "common.glsl"

// Static world-space lines: the front frame, floor rails and floor guides.
layout(location = 0) in vec3 a_pos;
uniform mat4 u_viewProj;

out float v_t;

void main() {
    v_t = clamp(tFromZ(a_pos.z), 0.0, 1.0);
    gl_Position = u_viewProj * vec4(a_pos, 1.0);
}

#version 300 es
precision highp float;

uniform vec3 u_color;
uniform float u_alpha;
uniform float u_fogStart;
uniform float u_fogPower;

in float v_t;
out vec4 outColor;

void main() {
    float fogX = clamp((v_t - u_fogStart) / (0.96 - u_fogStart), 0.0, 1.0);
    float fogVis = 1.0 - pow(fogX, u_fogPower);
    fogVis *= fogVis;
    outColor = vec4(u_color, u_alpha * fogVis);
}

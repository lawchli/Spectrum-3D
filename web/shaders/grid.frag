#version 300 es
precision highp float;

uniform sampler2D u_palette;
uniform vec3 u_lineColor;
uniform float u_linePalette;
uniform float u_lineFloor;
uniform float u_alpha;
uniform float u_fogStart;
uniform float u_fogPower;

in float v_level;
in float v_t;
in vec3 v_normal;
out vec4 outColor;

void main() {
    vec3 palette = texture(u_palette, vec2(mix(u_lineFloor, 1.0, v_level), 0.5)).rgb;
    vec3 color = mix(u_lineColor, palette, u_linePalette);
    float fogX = clamp((v_t - u_fogStart) / (0.96 - u_fogStart), 0.0, 1.0);
    float fogVis = 1.0 - pow(fogX, u_fogPower);
    fogVis *= fogVis;
    outColor = vec4(color, u_alpha * fogVis);
}

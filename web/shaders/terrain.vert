#version 300 es
precision highp float;
#include "common.glsl"

// a_grid: u (frequency), nominal age in data rows, data rows per mesh row (a power of two).
// Each mesh row holds one block of whole data rows, locked to absolute row numbers: as the
// waterfall scrolls, the row slides back with its data and then hands it to the next mesh
// row, instead of sliding over the data. A loud row therefore keeps its height all the way
// down, with no shimmer in the front view's silhouette and no gaps in the top view.
layout(location = 0) in vec3 a_grid;

uniform mat4 u_viewProj;
uniform float u_stepU;  // grid spacing in u, used for normals
uniform float u_lift;   // raises the grid lines a hair above the surface

out float v_level;
out float v_t;
out float v_band;  // semitone position, for the fragment shader (the axis may be warped, see bandToU)
out vec3 v_normal;

void main() {
    float u = a_grid.x;
    float band = u * (BANDS - 1.0);
    float stepRows = a_grid.z;
    float l = floor(log2(stepRows) + 0.5);
    float now = u_newest + u_phase;
    // The front row always shows the newest data row; the others the block they hold.
    float r = a_grid.y < 0.5 ? u_newest : stepRows * floor((now - a_grid.y) / stepRows);
    l = a_grid.y < 0.5 ? 0.0 : l;
    stepRows = a_grid.y < 0.5 ? 1.0 : stepRows;
    float age = a_grid.y < 0.5 ? 0.0 : now - r;
    float t = age / u_spanRows;

    float level = levelFromDb(blockDb(band, r, l));
    float stepBand = u_stepU * (BANDS - 1.0);
    float dl_u = levelFromDb(blockDb(band + stepBand, r, l)) - levelFromDb(blockDb(band - stepBand, r, l));
    // The block newer than the first two rows' may still be filling, so they use their own.
    float newer = a_grid.y < 1.5 * a_grid.z ? 0.0 : 1.0;
    float dl_t = levelFromDb(blockDb(band, r - stepRows, l)) - levelFromDb(blockDb(band, r + newer * stepRows, l));
    vec3 du = vec3((bandToU(band + stepBand) - bandToU(band - stepBand)) * u_size.x, dl_u * u_size.y, 0.0);
    vec3 dt = vec3(0.0, dl_t * u_size.y, -(1.0 + newer) * stepRows / u_spanRows * u_tail * u_size.z);

    v_level = level;
    v_t = t;
    v_band = band;
    v_normal = normalize(cross(du, dt));
    gl_Position = u_viewProj * vec4(worldPos(bandToU(band), level, t) + vec3(0.0, u_lift, 0.0), 1.0);
}

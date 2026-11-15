#version 450

// One window of the composition: a quad over the window and its shadow, placed in pixels.
// src/compose.rs fills the same block, field by field.
layout(push_constant) uniform Window {
    vec2 output_size;
    vec2 origin;
    vec2 size;
    vec2 shadow_offset;
    float corner_radius;
    float shadow_radius;
    float shadow_opacity;
} window;

// The pixel's place relative to the window's top-left corner.
layout(location = 0) out vec2 position;

void main() {
    // A strip of four vertices: (0, 0), (1, 0), (0, 1), (1, 1).
    vec2 corner = vec2(gl_VertexIndex & 1, gl_VertexIndex >> 1);
    vec2 reach = vec2(window.shadow_radius) + abs(window.shadow_offset);
    position = -reach + corner * (window.size + 2.0 * reach);
    vec2 pixel = window.origin + position;
    gl_Position = vec4(pixel / window.output_size * 2.0 - 1.0, 0.0, 1.0);
}

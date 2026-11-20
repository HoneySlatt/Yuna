#version 450

// A rectangle of the scene's output (src/scene.rs), placed in pixels: part of a window, or of the
// background. src/scene.rs fills the same block, field by field.
layout(push_constant) uniform Part {
    vec4 colour;
    vec2 output_size;
    vec2 origin;
    vec2 size;
    vec2 texture_origin;
    vec2 texture_size;
} part;

// The pixel's place from the rectangle's top-left corner.
layout(location = 0) out vec2 position;

void main() {
    // A strip of four vertices: (0, 0), (1, 0), (0, 1), (1, 1).
    vec2 corner = vec2(gl_VertexIndex & 1, gl_VertexIndex >> 1);
    position = corner * part.size;
    vec2 pixel = part.origin + position;
    gl_Position = vec4(pixel / part.output_size * 2.0 - 1.0, 0.0, 1.0);
}

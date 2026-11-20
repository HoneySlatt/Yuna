#version 450

// Part of a window: its client's texels, opaque, one to a pixel. As Smithay's GLES renderer
// draws an XRGB8888 buffer: sampled linearly, its alpha taken as 1, drawn without blending.

layout(push_constant) uniform Part {
    vec4 colour;
    vec2 output_size;
    vec2 origin;
    vec2 size;
    vec2 texture_origin;
    vec2 texture_size;
} part;

layout(set = 0, binding = 0) uniform sampler2D client_image;

layout(location = 0) in vec2 position;
layout(location = 0) out vec4 colour;

void main() {
    vec2 coordinates = (part.texture_origin + position) / part.texture_size;
    colour = vec4(textureLod(client_image, coordinates, 0.0).rgb, 1.0);
}

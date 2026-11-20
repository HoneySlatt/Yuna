#version 450

// Part of the background: one colour, as Smithay's GLES renderer clears what no window covers.

layout(push_constant) uniform Part {
    vec4 colour;
    vec2 output_size;
    vec2 origin;
    vec2 size;
    vec2 texture_origin;
    vec2 texture_size;
} part;

layout(location = 0) out vec4 colour;

void main() {
    colour = part.colour;
}

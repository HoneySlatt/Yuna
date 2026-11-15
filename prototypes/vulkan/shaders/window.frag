#version 450

// A window's pixel: its client's image with rounded corners over a soft shadow, premultiplied,
// which the pipeline blends over what is already drawn.

layout(push_constant) uniform Window {
    vec2 output_size;
    vec2 origin;
    vec2 size;
    vec2 shadow_offset;
    float corner_radius;
    float shadow_radius;
    float shadow_opacity;
} window;

layout(set = 0, binding = 0) uniform sampler2D client_image;

layout(location = 0) in vec2 position;
layout(location = 0) out vec4 colour;

// Signed distance from p to a rectangle centred on the origin with rounded corners.
float rounded_box(vec2 p, vec2 half_size, float radius) {
    vec2 q = abs(p) - half_size + radius;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
}

void main() {
    vec2 half_size = window.size * 0.5;
    vec2 centred = position - half_size;

    float distance = rounded_box(centred, half_size, window.corner_radius);
    // A one-pixel antialiased edge.
    float coverage = clamp(0.5 - distance, 0.0, 1.0);
    vec4 client = vec4(0.0);
    if (coverage > 0.0) {
        // An explicit level: implicit derivatives are undefined inside a branch.
        client = textureLod(client_image, position / window.size, 0.0) * coverage;
    }

    float shadow_distance = rounded_box(centred - window.shadow_offset, half_size, window.corner_radius);
    float shadow = window.shadow_opacity
        * (1.0 - smoothstep(-window.shadow_radius, window.shadow_radius, shadow_distance));

    colour = client + vec4(0.0, 0.0, 0.0, shadow) * (1.0 - client.a);
}

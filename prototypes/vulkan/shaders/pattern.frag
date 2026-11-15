#version 450

// The test pattern a client renders into its buffer: each pixel's channels encode its own
// position, so that a tiling or compression error shows as a wrong value at a known place.
// src/pattern.rs computes the same codes on the CPU; the two must change together.

layout(push_constant) uniform Pattern {
    // 0: 8-bit channels; 1: 10-bit colour and 2-bit alpha; 2: 16-bit floats.
    uint kind;
    // 1: alpha at its largest value, for windows; 0: alpha varies too, to check it survives.
    uint opaque;
} pattern;

layout(location = 0) out vec4 colour;

void main() {
    uvec2 p = uvec2(gl_FragCoord.xy);
    uvec4 code;
    vec4 scale;
    if (pattern.kind == 0u) {
        code = uvec4(p.x & 255u, p.y & 255u,
                     ((p.x >> 8) & 7u) | (((p.y >> 8) & 7u) << 3) | (((p.x ^ p.y) & 3u) << 6),
                     pattern.opaque == 1u ? 255u : (p.x + p.y) & 255u);
        scale = vec4(255.0);
    } else if (pattern.kind == 1u) {
        code = uvec4(p.x & 1023u, p.y & 1023u,
                     ((p.x >> 10) & 1u) | (((p.y >> 10) & 1u) << 1) | (((p.x * 3u + p.y * 5u) & 255u) << 2),
                     pattern.opaque == 1u ? 3u : (p.x + p.y) & 3u);
        scale = vec4(1023.0, 1023.0, 1023.0, 3.0);
    } else {
        code = uvec4(p.x & 2047u, p.y & 2047u, (p.x ^ p.y) & 2047u,
                     pattern.opaque == 1u ? 256u : (p.x + p.y) & 255u);
        scale = vec4(1.0, 1.0, 1.0, 256.0);
    }
    colour = vec4(code) / scale;
}

#version 100

// Yuna's area filter (ADR-0068), for an X11 surface drawn below XWayland's scale: each pixel is
// the average of the texels its footprint covers, weighted by the fraction of each it covers on
// each axis, a separable box. The captures' encoder averages the same way (`capture.rs`,
// `axis_taps`). It averages the values as stored, as that encoder does: in the buffer's own
// encoding, not in linear light.
//
// Smithay's contract for a texture shader (`compile_custom_texture_shader`): the three variants
// below (`EXTERNAL`, `NO_ALPHA`, `DEBUG_FLAGS`), `v_coords`, `tex`, `alpha` and `tint`.

//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

precision highp float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform float alpha;
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

// The texture's size, in texels.
uniform vec2 tex_size;
// Texels per pixel on each axis: 1 or more.
uniform vec2 ratio;

// The widest footprint averaged whole, in texels: a ratio of 1/4. A wider one is narrowed to its
// middle 4 texels on that axis, so that the loops below stay bounded, as GLSL ES 1.00 asks.
const float WIDEST = 4.0;
// The most texels a footprint of `WIDEST` touches on one axis, at any phase.
const int MAX_TAPS = 5;
// Where a texel is read from its corner: just past its centre. A sampler that rounds its
// position to 1/256 of a texel, down or to the nearest, then reads that texel alone, though the
// renderer filters bilinearly. The 1/1024 stays above the error of the division below and of the
// sampler's product, two roundings of a 32-bit float, for textures up to 8,000 texels a side.
const float INSIDE = 0.5 + 1.0 / 1024.0;

void main() {
    vec2 centre = v_coords * tex_size;
    vec2 half_span = 0.5 * min(ratio, vec2(WIDEST));
    vec2 start = max(centre - half_span, vec2(0.0));
    vec2 end = min(centre + half_span, tex_size);
    vec2 first = floor(start);

    vec4 sum = vec4(0.0);
    for (int j = 0; j < MAX_TAPS; j++) {
        float y = first.y + float(j);
        if (y >= end.y) {
            break;
        }
        float weight_y = min(end.y, y + 1.0) - max(start.y, y);
        vec4 row = vec4(0.0);
        for (int i = 0; i < MAX_TAPS; i++) {
            float x = first.x + float(i);
            if (x >= end.x) {
                break;
            }
            float weight_x = min(end.x, x + 1.0) - max(start.x, x);
            row += weight_x * texture2D(tex, vec2(x + INSIDE, y + INSIDE) / tex_size);
        }
        sum += weight_y * row;
    }
    // The weights add up to the footprint's area: dividing by it makes them add up to 1.
    vec2 span = max(end - start, vec2(1e-6));
    vec4 color = sum / (span.x * span.y);

#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0) * alpha;
#else
    color = color * alpha;
#endif

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}

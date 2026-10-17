#version 450
#extension GL_GOOGLE_include_directive : require
#include "rotation.glsl"
// Adapted from joemossjr16/artemis-android-pyrowave (GPL-3.0).
layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 frag;
layout(set = 0, binding = 0) uniform sampler2D u_y;
layout(set = 0, binding = 1) uniform sampler2D u_cb;
layout(set = 0, binding = 2) uniform sampler2D u_cr;
layout(push_constant) uniform Color { int ten_bit; int hdr; int full_range; int rotationQuarterTurns; } color;

void main() {
    vec2 uv = pyroSourceUv(v_uv, color.rotationQuarterTurns);
    vec2 cuv = uv;
    // The Rubylight host averages each 2x2 chroma block, so its sampling is centred.

    // The host normalizes 10-bit codes by 1023, rather than shifting them into P010.
    float peak = color.ten_bit != 0 ? 1023.0 : 255.0;
    float scale = color.ten_bit != 0 ? 4.0 : 1.0;
    float y = texture(u_y, uv).r;
    vec2 c = vec2(texture(u_cb, cuv).r, texture(u_cr, cuv).r) - (128.0 * scale / peak);
    if (color.full_range == 0) {
        y = (y - 16.0 * scale / peak) * peak / (219.0 * scale);
        c *= peak / (224.0 * scale);
    }
    vec3 rgb;
    if (color.hdr != 0) {
        // Inverse BT.2020 non-constant-luminance matrix; RGB remains PQ encoded.
        rgb = vec3(y + 1.4746 * c.y, y - 0.164553 * c.x - 0.571353 * c.y, y + 1.8814 * c.x);
    } else {
        rgb = vec3(y + 1.5748 * c.y, y - 0.187324 * c.x - 0.468124 * c.y, y + 1.8556 * c.x);
    }
    frag = vec4(clamp(rgb, 0.0, 1.0), 1.0);
}

#version 450
#extension GL_GOOGLE_include_directive : require

// Frame Warp (docs/frame-warp.md): video.frag with each pixel sampled at its position less the
// turn the host has not drawn yet, except where the host's HUD mask says the content stays put.
// Past the picture's edge the shifted position is mirrored back into it and darkened with the
// distance, rather than the edge column repeated as streaks (frame_warp.glsl and warp.frag alike).

// The sampler carries an immutable VkSamplerYcbcrConversion, as in video.frag.
layout(set = 0, binding = 0) uniform sampler2D uVideo;
// One byte per cell over the picture, rows from the top, set where the HUD stays put. Nearest,
// clamped to the edge.
layout(set = 1, binding = 0) uniform sampler2D uHud;

#include "video_common.glsl"

layout(location = 0) in vec2 vUv;
layout(location = 0) out vec4 outColor;

void main() {
    vec2 uv = vUv;
    float fade = 1.0;
    // The shift is the same for every pixel, so this branch is too: without one, no mask is read
    if (pc.warp.x != 0.0 || pc.warp.y != 0.0) {
        // 0 to 1 over the picture, which a decoder's padding can leave narrower than the crop drawn
        vec2 picture = (vUv - pc.warpRect.xy) / pc.warpRect.zw;
        // The mask's cell is the one this pixel is in, unshifted
        bool hud = false;
        if (pc.warp.z > 0.5) {
            hud = texture(uHud, picture).r > 0.5;
        }
        if (!hud) {
            vec2 p = picture - pc.warp.xy;
            vec2 outside = max(-p, p - 1.0);
            float edgeFade = 1.0 - 0.35 * clamp(max(max(outside.x, outside.y), 0.0) / 0.08, 0.0, 1.0);
            // Mirrored at the edges: -p below 0, 2 - p above 1
            p = 1.0 - abs(1.0 - abs(p));
            // Where the shifted position lands on the HUD, the scene behind it is not in the
            // picture: this pixel stays where it is rather than drawing the HUD a second time
            // beside itself.
            if (pc.warp.z <= 0.5 || texture(uHud, p).r <= 0.5) {
                fade = edgeFade;
                uv = pc.warpRect.xy + p * pc.warpRect.zw;
            }
        }
    }
    outColor = outputColor(texture(uVideo, clamp(uv, pc.uvClamp.xy, pc.uvClamp.zw)).rgb);
    outColor.rgb *= fade;
}

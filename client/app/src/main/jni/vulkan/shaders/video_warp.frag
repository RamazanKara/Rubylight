#version 450
#extension GL_GOOGLE_include_directive : require

// Frame Warp (docs/frame-warp.md): video.frag with each pixel sampled where the turn the host has
// not drawn yet brings it from, except where the host's HUD mask says the content stays put. The
// picture's centre moves by the shift and the rest is turned through the lens terms, as a camera
// turning shows it; a flat picture's terms leave every pixel at its position less the shift.
// Past the picture's edge the shifted position is taken back inside and darkened with the
// distance, and a pixel whose shifted position lands on the HUD looks back along the shift for the
// scene behind it, as the host's row says (pc.warpV2, frame_warp_v2_pc.h). nightjar_core's
// frame_warp_v2::edge_v2 and hud_probe are the references (frame_warp.glsl and warp.frag alike).

// The sampler carries an immutable VkSamplerYcbcrConversion, as in video.frag.
layout(set = 0, binding = 0) uniform sampler2D uVideo;
// One byte per cell over the picture, rows from the top, set where the HUD stays put. Nearest,
// clamped to the edge, as large as the host's mask.
layout(set = 1, binding = 0) uniform sampler2D uHud;

#include "video_common.glsl"

layout(location = 0) in vec2 vUv;
layout(location = 0) out vec4 outColor;

// The edge fill (frame_warp_v2::edge_v2): a turned position `p` past an edge taken back inside by a
// quarter of how far past it is, held inside the picture; `fade` darkens it by up to `strength`
// along a smoothstep over the first 8% of the picture past the edge.
vec2 frameWarpEdgeV2(vec2 p, float strength, out float fade) {
    vec2 outside = max(-p, p - 1.0);
    fade = 1.0 - clamp(strength, 0.0, 1.0) * smoothstep(0.0, 0.08, max(outside.x, outside.y));
    vec2 inside = mix(p, mix(-p * 0.25, 1.0 - (p - 1.0) * 0.25, greaterThan(p, vec2(1.0))),
                      greaterThan(outside, vec2(0.0)));
    return clamp(inside, 0.0, 1.0);
}

// The HUD probe (frame_warp_v2::hud_probe), for a turned position `p` whose fill lands on the HUD:
// up to pc.warpV2.y points one mask cell apart back along the shift, each through the edge fill.
// True with the first off the HUD in `at` and its brightness in `fade`.
bool frameWarpProbe(vec2 p, out vec2 at, out float fade) {
    // One mask cell along the shift's direction, per axis
    vec2 cell = normalize(pc.warp.xy) / vec2(textureSize(uHud, 0));
    for (int k = 1; k <= 8; ++k) {
        if (float(k) > pc.warpV2.y) break;
        at = frameWarpEdgeV2(p - float(k) * cell, pc.warpV2.x, fade);
        if (texture(uHud, at).r <= 0.5) return true;
    }
    return false;
}

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
            // From the picture's centre, turned through the lens terms (nightjar_core's
            // frame_warp::source): picture - pc.warp.xy for a flat picture
            vec2 q = picture - 0.5;
            vec2 p = 0.5 + (q * pc.warpLens.zw - pc.warp.xy) / max(1.0 + dot(q, pc.warpLens.xy), 0.25);
            float edgeFade;
            vec2 shifted = frameWarpEdgeV2(p, pc.warpV2.x, edgeFade);
            // Where the shifted position lands on the HUD, the scene behind it is not in the
            // picture: the probe looks for it a few cells back along the shift, and without it this
            // pixel stays where it is rather than drawing the HUD a second time beside itself.
            bool found = pc.warp.z <= 0.5 || texture(uHud, shifted).r <= 0.5;
            if (!found) {
                found = frameWarpProbe(p, shifted, edgeFade);
            }
            if (found) {
                fade = edgeFade;
                uv = pc.warpRect.xy + shifted * pc.warpRect.zw;
            }
        }
    }
    outColor = outputColor(texture(uVideo, clamp(uv, pc.uvClamp.xy, pc.uvClamp.zw)).rgb);
    outColor.rgb *= fade;
}

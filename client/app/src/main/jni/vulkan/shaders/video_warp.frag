#version 450
#extension GL_GOOGLE_include_directive : require

// Frame Warp (docs/frame-warp.md): video.frag with each pixel sampled at its position less the
// turn the host has not drawn yet, except where the host's HUD mask says the content stays put.

// The sampler carries an immutable VkSamplerYcbcrConversion, as in video.frag.
layout(set = 0, binding = 0) uniform sampler2D uVideo;
// One byte per cell over the picture, rows from the top, set where the HUD stays put. Nearest,
// clamped to the edge.
layout(set = 1, binding = 0) uniform sampler2D uHud;

#include "video_common.glsl"

layout(location = 0) in vec2 vUv;
layout(location = 0) out vec4 outColor;

void main() {
    // The mask's cell is the one this pixel is in, unshifted
    vec2 picture = (vUv - pc.uvRect.xy) / pc.uvRect.zw;
    bool hud = pc.warp.z > 0.5 && texture(uHud, picture).r > 0.5;
    vec2 uv = clamp(hud ? vUv : vUv - pc.warp.xy, pc.uvClamp.xy, pc.uvClamp.zw);
    outColor = outputColor(texture(uVideo, uv).rgb);
}

// Frame Warp (docs/frame-warp.md), for the pyrowave-renderer shaders built with -DFRAME_WARP: the
// picture is sampled at its position less the turn the host has not drawn yet, except where the
// host's HUD mask keeps it in place; edges clamp. As the Vulkan video path (video_warp.frag), in
// the source picture's coordinates with the origin top left (pyroSourceUv).
// One byte per mask cell over the picture, rows from the top, set where the HUD stays put.
layout(set = 1, binding = 0) uniform sampler2D u_hud;
layout(set = 1, binding = 1) uniform FrameWarpShift {
    // xy: the shift as fractions of the picture, x right and y down; z: 1 to keep the HUD in place
    vec4 shift;
} frameWarp;

vec2 frameWarpUv(vec2 uv) {
    float cell = texture(u_hud, uv).r;
    bool hud = frameWarp.shift.z > 0.5 && cell > 0.5;
    return hud ? uv : clamp(uv - frameWarp.shift.xy, 0.0, 1.0);
}

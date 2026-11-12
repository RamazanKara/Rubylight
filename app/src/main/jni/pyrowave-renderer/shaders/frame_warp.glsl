// Frame Warp (docs/frame-warp.md), for the pyrowave-renderer shaders built with -DFRAME_WARP: the
// picture is sampled at its position less the turn the host has not drawn yet, except where the
// host's HUD mask keeps it in place. Past an edge the shifted position is mirrored back into the
// picture and the colour darkened with the distance, rather than the edge column repeated as
// streaks. As the Vulkan video path (video_warp.frag), in the source picture's coordinates with the
// origin top left (pyroSourceUv).
// One byte per mask cell over the picture, rows from the top, set where the HUD stays put.
layout(set = 1, binding = 0) uniform sampler2D u_hud;
layout(set = 1, binding = 1) uniform FrameWarpShift {
    // xy: the shift as fractions of the picture, x right and y down; z: 1 to keep the HUD in place
    vec4 shift;
} frameWarp;

// The position to sample for `uv`, and in `fade` what to multiply the colour by
vec2 frameWarpUv(vec2 uv, out float fade) {
    fade = 1.0;
    // The shift is the same for every pixel, so these branches are too: no shift reads no mask, and
    // the mask is read only when it applies
    if (frameWarp.shift.x == 0.0 && frameWarp.shift.y == 0.0) return uv;
    if (frameWarp.shift.z > 0.5) {
        if (texture(u_hud, uv).r > 0.5) return uv;
    }
    vec2 p = uv - frameWarp.shift.xy;
    vec2 outside = max(-p, p - 1.0);
    // Mirrored at the edges: -p below 0, 2 - p above 1
    vec2 source = clamp(1.0 - abs(1.0 - abs(p)), 0.0, 1.0);
    // Where the shifted position lands on the HUD, the scene behind it is not in the picture: this
    // pixel stays where it is rather than drawing the HUD a second time beside itself.
    if (frameWarp.shift.z > 0.5 && texture(u_hud, source).r > 0.5) return uv;
    fade = 1.0 - 0.35 * clamp(max(max(outside.x, outside.y), 0.0) / 0.08, 0.0, 1.0);
    return source;
}

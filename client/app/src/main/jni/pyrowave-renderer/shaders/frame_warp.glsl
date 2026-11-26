// Frame Warp (docs/frame-warp.md), for the pyrowave-renderer shaders built with -DFRAME_WARP: the
// picture is sampled where the turn the host has not drawn yet brings each pixel from, except where
// the host's HUD mask keeps it in place. The picture's centre moves by the shift and the rest is
// turned through the lens terms, as a camera turning shows it; a flat picture's terms leave every
// pixel at its position less the shift. Past an edge the shifted position is taken back inside and
// darkened with the distance, and a pixel whose shifted position lands on the HUD looks back along
// the shift for the scene behind it, as the host's row says (frameWarp.v2,
// vulkan/frame_warp_v2_pc.h). As the Vulkan video path (video_warp.frag), in the source picture's
// coordinates with the origin top left (pyroSourceUv); nightjar_core's frame_warp_v2::edge_v2 and
// hud_probe are the references.
// One byte per mask cell over the picture, rows from the top, set where the HUD stays put; as large
// as the host's mask.
layout(set = 1, binding = 0) uniform sampler2D u_hud;
layout(set = 1, binding = 1) uniform FrameWarpShift {
    // xy: the shift of the picture's centre as fractions of the picture, x right and y down; z: 1
    // to keep the HUD in place
    vec4 shift;
    // The lens terms the rest of the picture is turned through: xy = k, zw = n (0, 0, 1, 1 for a
    // flat picture)
    vec4 lens;
    // x: the edge fill's strength, y: how many mask cells the HUD probe looks back; z, w for
    // information only
    vec4 v2;
} frameWarp;

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
// up to frameWarp.v2.y points one mask cell apart back along the shift, each through the edge
// fill. The first off the HUD with its brightness in `fade`, or `uv` as it is.
vec2 frameWarpProbe(vec2 uv, vec2 p, out float fade) {
    fade = 1.0;
    // One mask cell along the shift's direction, per axis
    vec2 cell = normalize(frameWarp.shift.xy) / vec2(textureSize(u_hud, 0));
    for (int k = 1; k <= 8; ++k) {
        if (float(k) > frameWarp.v2.y) break;
        float probeFade;
        vec2 at = frameWarpEdgeV2(p - float(k) * cell, frameWarp.v2.x, probeFade);
        if (texture(u_hud, at).r <= 0.5) {
            fade = probeFade;
            return at;
        }
    }
    return uv;
}

// The position to sample for `uv`, and in `fade` what to multiply the colour by
vec2 frameWarpUv(vec2 uv, out float fade) {
    fade = 1.0;
    // The shift is the same for every pixel, so these branches are too: no shift reads no mask, and
    // the mask is read only when it applies
    if (frameWarp.shift.x == 0.0 && frameWarp.shift.y == 0.0) return uv;
    if (frameWarp.shift.z > 0.5) {
        if (texture(u_hud, uv).r > 0.5) return uv;
    }
    // From the picture's centre, turned through the lens terms (nightjar_core's
    // frame_warp::source): uv - frameWarp.shift.xy for a flat picture
    vec2 q = uv - 0.5;
    vec2 p = 0.5 + (q * frameWarp.lens.zw - frameWarp.shift.xy) / max(1.0 + dot(q, frameWarp.lens.xy), 0.25);
    float edgeFade;
    vec2 source = frameWarpEdgeV2(p, frameWarp.v2.x, edgeFade);
    // Where the shifted position lands on the HUD, the scene behind it is not in the picture: the
    // probe looks for it a few cells back along the shift, and without it this pixel stays where it
    // is rather than drawing the HUD a second time beside itself.
    if (frameWarp.shift.z > 0.5 && texture(u_hud, source).r > 0.5) return frameWarpProbe(uv, p, fade);
    fade = edgeFade;
    return source;
}

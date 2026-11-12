// Frame Warp (stream/frame_warp.rs), one definition for every HLSL presenter. A shader includes
// it after declaring `warp`, its constants' row (the shift in fractions of the picture, then the
// HUD mask's size in cells, 0 for none; all zero draws the picture where it is), and `hudMask`
// (byte x / 8 of row y holds cell x in bit x % 8). glslang reads this file from beside the
// shader; the Direct3D 11 build splices it in (`frame_warp::hlsl`). Positions are in any unit in
// which the picture spans 0 to `size`: its pixels, or a texture's coordinates.

// Whether the point `q` of the picture is on a set cell of the HUD mask (the nearest cell past
// its edge).
bool frameWarpHud(float2 q, float2 size) {
    if (warp.z <= 0) return false;
    uint2 cell = min(uint2(saturate(q / size) * warp.zw), uint2(warp.zw) - 1);
    return ((uint(hudMask.Load(int3(cell.x >> 3, cell.y, 0)) * 255 + 0.5) >> (cell.x & 7)) & 1) != 0;
}

// The edge fill (nightjar_core::frame_warp::edge): where the shifted point `q` samples the
// picture, mirrored at its edges so a turn uncovers a reflection rather than streaks of the edge,
// and in `fade` how bright it is drawn there: up to 35% darker within 8% of the picture past the
// edge. Inside the picture `q` itself at full brightness. The caller keeps the sample inside the
// picture's texels.
float2 frameWarpEdge(float2 q, float2 size, out float fade) {
    float2 outside = max(-q, q - size) / size;
    fade = 1 - 0.35 * saturate(max(outside.x, outside.y) / 0.08);
    return min(abs(q), 2 * size - q);
}

// Where the output point `q` samples the picture under a shift of `shift` (the same unit), and
// in `fade` how bright: `q` itself on a HUD cell or where its shifted source lands on one, else
// `q` less the shift with the edge fill.
float2 frameWarpShifted(float2 q, float2 size, float2 shift, out float fade) {
    fade = 1;
    if (frameWarpHud(q, size)) return q;
    float edgeFade;
    float2 source = frameWarpEdge(q - shift, size, edgeFade);
    // A source on the HUD would draw the HUD a second time beside itself, a shift away. The scene
    // behind the HUD isn't in the picture, so the point keeps its own unshifted pixel there.
    if (frameWarpHud(source, size)) return q;
    fade = edgeFade;
    return source;
}

// `frameWarpShifted` by the row's shift; `q` as it is, at full brightness, for an all-zero row.
float2 frameWarp(float2 q, float2 size, out float fade) {
    fade = 1;
    if (all(warp == 0)) return q;
    return frameWarpShifted(q, size, warp.xy * size, fade);
}

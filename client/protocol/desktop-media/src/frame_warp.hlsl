// Frame Warp (stream/frame_warp.rs), one definition for every HLSL presenter. A shader includes
// it after declaring three rows of its constants, `warp` (the shift of the picture's centre in
// fractions of the picture, then the HUD mask's size in cells, 0 for none; all zero draws the
// picture where it is), `warpLens` (the lens terms, `nightjar_core::frame_warp::lens_terms`:
// 0, 0, 1, 1 for a picture that moves alike everywhere) and `warpV2` (stream/frame_warp_v2.rs:
// the edge fill's strength from 0 to 1 and the HUD probe's reach in mask cells, 0 for none; the
// confidence and flags after them only the CPU reads), and `hudMask` (byte x / 8 of row y holds
// cell x in bit x % 8). glslang reads this file from beside the shader; the Direct3D 11 build
// splices it in (`frame_warp::hlsl`). Positions are in any unit in which the picture spans 0 to
// `size`: its pixels, or a texture's coordinates.

// Whether the point `q` of the picture is on a set cell of the HUD mask (the nearest cell past
// its edge).
bool frameWarpHud(float2 q, float2 size) {
    if (warp.z <= 0) return false;
    uint2 cell = min(uint2(saturate(q / size) * warp.zw), uint2(warp.zw) - 1);
    return ((uint(hudMask.Load(int3(cell.x >> 3, cell.y, 0)) * 255 + 0.5) >> (cell.x & 7)) & 1) != 0;
}

// One axis of `frameWarpEdgeV2`: back inside by a quarter of how far past the edge `v` is.
float frameWarpReflectV2(float v, float size) {
    if (v < 0) return -v * 0.25;
    if (v > size) return size - (v - size) * 0.25;
    return v;
}

// The edge fill (nightjar_core::frame_warp_v2::edge_v2): where the shifted point `q` samples the
// picture and in `fade` how bright it is drawn there. Past an edge it samples back inside by a
// quarter of how far past the edge it is (a squeezed reflection that reads as the edge
// stretching, never a second copy of what crosses it), darkened by up to the strength `warpV2.x`
// along a smoothstep over 8% of the picture. Inside the picture `q` itself at full brightness.
// The caller keeps the sample inside the picture's texels.
float2 frameWarpEdgeV2(float2 q, float2 size, out float fade) {
    float2 outside = max(-q, q - size) / size;
    float d = saturate(max(outside.x, outside.y) / 0.08);
    fade = 1 - saturate(warpV2.x) * d * d * (3 - 2 * d);
    return clamp(float2(frameWarpReflectV2(q.x, size.x), frameWarpReflectV2(q.y, size.y)), 0, size);
}

// The HUD probe (nightjar_core::frame_warp_v2::hud_probe): where `source`, the edge fill of
// `unfilled`, lands on the HUD, the first of up to `warpV2.y` (at most 8) points one mask cell
// apart back along the shift (`unfilled - k * step`, `step` the shift's direction one cell long on
// each axis) whose edge fill is off the HUD, with its brightness in `fade`. Without one `source`
// stays as it is, on the HUD.
float2 frameWarpProbe(float2 unfilled, float2 source, float2 size, float2 shift, inout float fade) {
    if (!frameWarpHud(source, size)) return source;
    float2 direction = shift / size;
    float reach = length(direction);
    if (reach <= 0 || any(warp.zw <= 0)) return source;
    float2 step = direction / reach / warp.zw * size;
    for (int k = 1; k <= 8; k++) {
        if (k > warpV2.y) break;
        float probeFade;
        float2 probe = frameWarpEdgeV2(unfilled - k * step, size, probeFade);
        if (!frameWarpHud(probe, size)) {
            fade = probeFade;
            return probe;
        }
    }
    return source;
}

// Whether the picture moves alike everywhere: no lens, so a shift only slides it.
bool frameWarpFlat() {
    return all(warpLens == float4(0, 0, 1, 1));
}

// Where the output point `q` samples the picture, before the edge fill, when its centre is
// shifted by `shift` (the same unit) (nightjar_core::frame_warp::source): `q` less the shift for
// a flat picture, to the bit; through a lens, turned as a camera turning shows it, the edges
// farther than the centre.
float2 frameWarpSource(float2 q, float2 size, float2 shift) {
    if (frameWarpFlat()) return q - shift;
    float2 c = q / size - 0.5;
    return (0.5 + (c * warpLens.zw - shift / size) / max(1 + dot(c, warpLens.xy), 0.25)) * size;
}

// Where the output point `q` samples the picture under a shift of `shift` at its centre (the
// same unit), and in `fade` how bright: `q` itself on a HUD cell or where its shifted source
// lands on one and the probe finds no scene beside it, else that source (`frameWarpSource`) with
// the edge fill, or what the probe found.
float2 frameWarpShifted(float2 q, float2 size, float2 shift, out float fade) {
    fade = 1;
    if (frameWarpHud(q, size)) return q;
    float edgeFade;
    float2 unfilled = frameWarpSource(q, size, shift);
    float2 source = frameWarpEdgeV2(unfilled, size, edgeFade);
    // A source on the HUD would draw the HUD a second time beside itself, a shift away. The probe
    // looks back along the shift for the scene beside the HUD; the scene behind it isn't in the
    // picture, so without that the point keeps its own unshifted pixel.
    if (warpV2.y > 0) source = frameWarpProbe(unfilled, source, size, shift, edgeFade);
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

// Undo the clockwise surface pre-transform to sample the unrotated video.
// Coordinates use Vulkan's top-left image convention. Filtering remains in
// source image coordinates, including SGSR's pixel neighbourhood calculations.
vec2 pyroSourceUv(vec2 uv, int quarterTurns) {
    if (quarterTurns == 1) return vec2(uv.y, 1.0 - uv.x);
    if (quarterTurns == 2) return vec2(1.0) - uv;
    if (quarterTurns == 3) return vec2(1.0 - uv.y, uv.x);
    return uv;
}

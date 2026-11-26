#version 300 es
#extension GL_OES_EGL_image_external_essl3 : require
// Frame Warp (docs/frame-warp.md): the picture sampled where the turn the PC has not drawn yet
// brings each pixel from, except where the HUD mask says the content stays put. The picture's
// centre moves by the shift and the rest is turned through the lens terms, as a camera turning
// shows it; a flat picture's terms leave every pixel at its position less the shift. Past an edge
// the shifted position is taken back inside and darkened with the distance, and a pixel whose
// shifted position lands on the HUD looks back along the shift for the scene behind it, as the
// PC's row says (uWarpV2; video_warp.frag and frame_warp.glsl alike, nightjar_core's
// frame_warp_v2::edge_v2 and hud_probe the references). uv runs 0 to 1 over the picture with y up
// (upscale.vert); the shift, the lens terms and the mask have y down.
precision highp float;
uniform highp samplerExternalOES source;
uniform mat4 textureTransform;
// xy: the shift of the picture's centre as fractions of its width and height; z: 1 to keep the mask's cells unshifted.
uniform vec3 warp;
// The lens terms the rest of the picture is turned through: xy = k, zw = n (0, 0, 1, 1 for a flat picture).
uniform vec4 lens;
// GlesFrameWarpV2: x = the edge fill's strength, y = how many mask cells the HUD probe looks back; z, w for information only.
uniform vec4 uWarpV2;
// One byte per cell, rows from the top: set where the HUD stays put. Nearest, clamped, as large as the PC's mask.
uniform lowp sampler2D hudMask;
in vec2 uv;
out vec4 color;

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
// up to uWarpV2.y points one mask cell apart back along the shift, each through the edge fill.
// True with the first off the HUD in `at` and its brightness in `fade`.
bool frameWarpProbe(vec2 p, out vec2 at, out float fade) {
    // One mask cell along the shift's direction, per axis
    vec2 cell = normalize(warp.xy) / vec2(textureSize(hudMask, 0));
    for (int k = 1; k <= 8; ++k) {
        if (float(k) > uWarpV2.y) break;
        at = frameWarpEdgeV2(p - float(k) * cell, uWarpV2.x, fade);
        if (texture(hudMask, at).r <= 0.5) return true;
    }
    return false;
}

void main() {
    vec2 at = uv;
    float fade = 1.0;
    // The shift is the same for every pixel, so this branch is too: without one, no mask is read
    if (warp.x != 0.0 || warp.y != 0.0) {
        vec2 picture = vec2(uv.x, 1.0 - uv.y);
        bool hud = false;
        if (warp.z > 0.5) {
            hud = texture(hudMask, picture).r > 0.5;
        }
        if (!hud) {
            // From the picture's centre, turned through the lens terms (nightjar_core's
            // frame_warp::source): picture - warp.xy for a flat picture
            vec2 q = picture - 0.5;
            vec2 p = 0.5 + (q * lens.zw - warp.xy) / max(1.0 + dot(q, lens.xy), 0.25);
            float edgeFade;
            vec2 shifted = frameWarpEdgeV2(p, uWarpV2.x, edgeFade);
            // Where the shifted position lands on the HUD, the scene behind it is not in the
            // picture: the probe looks for it a few cells back along the shift, and without it this
            // pixel stays where it is rather than drawing the HUD a second time beside itself.
            bool found = warp.z <= 0.5 || texture(hudMask, shifted).r <= 0.5;
            if (!found) {
                found = frameWarpProbe(p, shifted, edgeFade);
            }
            if (found) {
                fade = edgeFade;
                at = vec2(shifted.x, 1.0 - shifted.y);
            }
        }
    }
    color = vec4(texture(source, (textureTransform * vec4(at, 0.0, 1.0)).xy).rgb * fade, 1.0);
}

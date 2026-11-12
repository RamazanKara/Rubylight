#version 300 es
#extension GL_OES_EGL_image_external_essl3 : require
// Frame Warp (docs/frame-warp.md): the picture sampled where the turn the PC has not drawn yet
// brings each pixel from, except where the HUD mask says the content stays put. The picture's
// centre moves by the shift and the rest is turned through the lens terms, as a camera turning
// shows it; a flat picture's terms leave every pixel at its position less the shift. Past an edge
// the shifted position is mirrored back into the picture and darkened with the distance, rather
// than the edge column repeated as streaks (video_warp.frag and frame_warp.glsl alike). uv runs 0
// to 1 over the picture with y up (upscale.vert); the shift, the lens terms and the mask have y
// down.
precision highp float;
uniform highp samplerExternalOES source;
uniform mat4 textureTransform;
// xy: the shift of the picture's centre as fractions of its width and height; z: 1 to keep the mask's cells unshifted.
uniform vec3 warp;
// The lens terms the rest of the picture is turned through: xy = k, zw = n (0, 0, 1, 1 for a flat picture).
uniform vec4 lens;
// One byte per cell, rows from the top: set where the HUD stays put. Nearest, clamped.
uniform lowp sampler2D hudMask;
in vec2 uv;
out vec4 color;

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
            vec2 outside = max(-p, p - 1.0);
            float edgeFade = 1.0 - 0.35 * clamp(max(max(outside.x, outside.y), 0.0) / 0.08, 0.0, 1.0);
            // Mirrored at the edges: -p below 0, 2 - p above 1
            p = 1.0 - abs(1.0 - abs(p));
            // Where the shifted position lands on the HUD, the scene behind it is not in the
            // picture: this pixel stays where it is rather than drawing the HUD a second time
            // beside itself.
            if (warp.z <= 0.5 || texture(hudMask, p).r <= 0.5) {
                fade = edgeFade;
                at = clamp(vec2(p.x, 1.0 - p.y), 0.0, 1.0);
            }
        }
    }
    color = vec4(texture(source, (textureTransform * vec4(at, 0.0, 1.0)).xy).rgb * fade, 1.0);
}

#version 300 es
#extension GL_OES_EGL_image_external_essl3 : require
// Frame Warp (docs/frame-warp.md): the picture sampled at its position less the shift the PC has
// not drawn yet, except where the HUD mask says the content stays put; edges clamp. uv runs 0 to 1
// over the picture with y up (upscale.vert); the shift and the mask have y down.
precision highp float;
uniform highp samplerExternalOES source;
uniform mat4 textureTransform;
// xy: the shift as fractions of the picture's width and height; z: 1 to keep the mask's cells unshifted.
uniform vec3 warp;
// One byte per cell, rows from the top: set where the HUD stays put. Nearest, clamped.
uniform lowp sampler2D hudMask;
in vec2 uv;
out vec4 color;

void main() {
    vec2 picture = vec2(uv.x, 1.0 - uv.y);
    bool hud = warp.z > 0.5 && texture(hudMask, picture).r > 0.5;
    vec2 at = hud ? uv : clamp(uv - vec2(warp.x, -warp.y), 0.0, 1.0);
    color = vec4(texture(source, (textureTransform * vec4(at, 0.0, 1.0)).xy).rgb, 1.0);
}

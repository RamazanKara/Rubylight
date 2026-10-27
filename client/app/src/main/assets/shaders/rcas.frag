#version 300 es
// AMD FidelityFX FSR1, Copyright (c) 2021 Advanced Micro Devices, Inc.
// MIT license and upstream revision: ../NOTICE-FSR1.txt.
// FP32 RCAS port using native highp arithmetic and clamped edge loads.
precision highp float;
precision highp int;
uniform highp sampler2D source;
uniform float sharpness;
in vec2 uv;
out vec4 color;

vec3 FsrRcasLoadF(ivec2 p) {
    return texelFetch(source, clamp(p, ivec2(0), textureSize(source, 0) - 1), 0).rgb;
}

vec3 FsrRcasF(ivec2 p) {
    vec3 b = FsrRcasLoadF(p + ivec2(0, -1));
    vec3 d = FsrRcasLoadF(p + ivec2(-1, 0));
    vec3 e = FsrRcasLoadF(p);
    vec3 f = FsrRcasLoadF(p + ivec2(1, 0));
    vec3 h = FsrRcasLoadF(p + ivec2(0, 1));
    vec3 mn4 = min(min(b, d), min(f, h));
    vec3 mx4 = max(max(b, d), max(f, h));
    // Keep black/white plateaus finite on GLES implementations that propagate 0/0.
    vec3 hitMin = min(mn4, e) / max(4.0 * mx4, vec3(1.0e-8));
    vec3 hitMax = (1.0 - max(mx4, e)) / min(4.0 * mn4 - 4.0, vec3(-1.0e-8));
    vec3 lobes = max(-hitMin, hitMax);
    float lobe = max(-(0.25 - 1.0 / 16.0), min(max(max(lobes.r, lobes.g), lobes.b), 0.0)) * sharpness;
    return (lobe * (b + d + f + h) + e) / (4.0 * lobe + 1.0);
}

void main() {
    color = vec4(FsrRcasF(ivec2(uv * vec2(textureSize(source, 0)))), 1.0);
}

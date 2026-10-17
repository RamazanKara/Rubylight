#version 450
#extension GL_GOOGLE_include_directive : require
#include "rotation.glsl"
// Copyright (c) 2025, Qualcomm Innovation Center, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause
// Vulkan port of assets/shaders/sgsr.frag (SGSR 1 mobile port; see assets/NOTICE-SGSR1.txt),
// reading PyroWave's 8-bit SDR YCbCr planes directly so colour conversion and upscaling are
// one pass. Edges are found on Y instead of RGB green; adding the same delta to R, G and B
// shifts BT.709 luma by exactly that delta. The math runs in the GLES port's bottom-left
// image space, so both ports filter identically.
layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 color;
layout(set = 0, binding = 0) uniform sampler2D u_y;
layout(set = 0, binding = 1) uniform sampler2D u_cb;
layout(set = 0, binding = 2) uniform sampler2D u_cr;
layout(push_constant) uniform Params {
    vec4 viewportInfo;
    float edgeSharpness;
    int fullRange;
    int rotationQuarterTurns;
} params;

vec2 planeCoord(vec2 p) {
    p = clamp(p, 0.5 * params.viewportInfo.xy, vec2(1.0) - 0.5 * params.viewportInfo.xy);
    return vec2(p.x, 1.0 - p.y);
}

float expandY(float y) {
    return params.fullRange != 0 ? y : (y - 16.0 / 255.0) * (255.0 / 219.0);
}

float loadGreen(vec2 pixel) {
    return expandY(texture(u_y, planeCoord((pixel + 0.5) * params.viewportInfo.xy)).r);
}

vec3 loadColor(vec2 p) {
    vec2 uv = planeCoord(p);
    float y = expandY(texture(u_y, uv).r);
    vec2 c = vec2(texture(u_cb, uv).r, texture(u_cr, uv).r) - 128.0 / 255.0;
    if (params.fullRange == 0) c *= 255.0 / 224.0;
    return clamp(vec3(y + 1.5748 * c.y, y - 0.187324 * c.x - 0.468124 * c.y, y + 1.8556 * c.x), 0.0, 1.0);
}

float fastLanczos2(float x) {
    float wA = x - 4.0;
    float wB = x * wA - wA;
    wA *= wA;
    return wB * wA;
}

vec2 weightY(float dx, float dy, float c, float std) {
    float x = (dx * dx + dy * dy) * 0.55 + clamp(abs(c) * std, 0.0, 1.0);
    float w = fastLanczos2(x);
    return vec2(w, w * c);
}

void main() {
    vec2 sourceUv = pyroSourceUv(v_uv, params.rotationQuarterTurns);
    vec2 uv = vec2(sourceUv.x, 1.0 - sourceUv.y);
    vec3 rgb = loadColor(uv);
    float centerY = expandY(texture(u_y, planeCoord(uv)).r);
    vec2 imgCoord = uv * params.viewportInfo.zw + vec2(-0.5, 0.5);
    vec2 pixel = floor(imgCoord);
    vec2 pl = imgCoord - pixel;

    vec4 left = vec4(loadGreen(pixel + vec2(-1.0, 0.0)), loadGreen(pixel),
                     loadGreen(pixel + vec2(0.0, -1.0)), loadGreen(pixel + vec2(-1.0, -1.0)));
    float edgeVote = abs(left.z - left.y) + abs(centerY - left.y) + abs(centerY - left.z);
    if (edgeVote > 8.0 / 255.0) {
        vec4 right = vec4(loadGreen(pixel + vec2(1.0, 0.0)), loadGreen(pixel + vec2(2.0, 0.0)),
                         loadGreen(pixel + vec2(2.0, -1.0)), loadGreen(pixel + vec2(1.0, -1.0)));
        vec4 upDown = vec4(loadGreen(pixel + vec2(0.0, -2.0)), loadGreen(pixel + vec2(1.0, -2.0)),
                          loadGreen(pixel + vec2(1.0, 1.0)), loadGreen(pixel + vec2(0.0, 1.0)));
        float mean = (left.y + left.z + right.x + right.w) * 0.25;
        left -= mean;
        right -= mean;
        upDown -= mean;
        float center = centerY - mean;
        float sum = dot(abs(left) + abs(right) + abs(upDown), vec4(1.0));
        float std = 2.181818 / max(sum, 1.0e-8);

        vec2 aWY = weightY(pl.x, pl.y + 1.0, upDown.x, std);
        aWY += weightY(pl.x - 1.0, pl.y + 1.0, upDown.y, std);
        aWY += weightY(pl.x - 1.0, pl.y - 2.0, upDown.z, std);
        aWY += weightY(pl.x, pl.y - 2.0, upDown.w, std);
        aWY += weightY(pl.x + 1.0, pl.y - 1.0, left.x, std);
        aWY += weightY(pl.x, pl.y - 1.0, left.y, std);
        aWY += weightY(pl.x, pl.y, left.z, std);
        aWY += weightY(pl.x + 1.0, pl.y, left.w, std);
        aWY += weightY(pl.x - 1.0, pl.y - 1.0, right.x, std);
        aWY += weightY(pl.x - 2.0, pl.y - 1.0, right.y, std);
        aWY += weightY(pl.x - 2.0, pl.y, right.z, std);
        aWY += weightY(pl.x - 1.0, pl.y, right.w, std);

        float finalY = aWY.y / max(aWY.x, 1.0e-8);
        float maxY = max(max(left.y, left.z), max(right.x, right.w));
        float minY = min(min(left.y, left.z), min(right.x, right.w));
        finalY = clamp(params.edgeSharpness * finalY, minY, maxY);
        float deltaY = clamp(finalY - center, -23.0 / 255.0, 23.0 / 255.0);
        rgb = clamp(rgb + deltaY, 0.0, 1.0);
    }
    color = vec4(rgb, 1.0);
}

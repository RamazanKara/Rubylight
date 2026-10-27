#version 300 es
#extension GL_OES_EGL_image_external_essl3 : require
// Copyright (c) 2025, Qualcomm Innovation Center, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause
// SGSR 1 mobile RGBA port; license and upstream revision: ../NOTICE-SGSR1.txt.
// Explicit OES taps replace gathers; highp keeps coordinates and low-contrast divisions finite.
precision highp float;
uniform highp samplerExternalOES source;
uniform mat4 textureTransform;
uniform vec4 viewportInfo;
uniform float edgeSharpness;
in vec2 uv;
out vec4 color;

vec3 loadColor(vec2 p) {
    p = clamp(p, 0.5 * viewportInfo.xy, vec2(1.0) - 0.5 * viewportInfo.xy);
    return texture(source, (textureTransform * vec4(p, 0.0, 1.0)).xy).rgb;
}

float loadGreen(vec2 pixel) {
    return loadColor((pixel + 0.5) * viewportInfo.xy).g;
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
    vec3 rgb = loadColor(uv);
    vec2 imgCoord = uv * viewportInfo.zw + vec2(-0.5, 0.5);
    vec2 pixel = floor(imgCoord);
    vec2 pl = imgCoord - pixel;

    // GLSL gather order is upper-left, upper-right, lower-right, lower-left.
    vec4 left = vec4(loadGreen(pixel + vec2(-1.0, 0.0)), loadGreen(pixel),
                     loadGreen(pixel + vec2(0.0, -1.0)), loadGreen(pixel + vec2(-1.0, -1.0)));
    float edgeVote = abs(left.z - left.y) + abs(rgb.g - left.y) + abs(rgb.g - left.z);
    if (edgeVote > 8.0 / 255.0) {
        vec4 right = vec4(loadGreen(pixel + vec2(1.0, 0.0)), loadGreen(pixel + vec2(2.0, 0.0)),
                         loadGreen(pixel + vec2(2.0, -1.0)), loadGreen(pixel + vec2(1.0, -1.0)));
        vec4 upDown = vec4(loadGreen(pixel + vec2(0.0, -2.0)), loadGreen(pixel + vec2(1.0, -2.0)),
                          loadGreen(pixel + vec2(1.0, 1.0)), loadGreen(pixel + vec2(0.0, 1.0)));
        float mean = (left.y + left.z + right.x + right.w) * 0.25;
        left -= mean;
        right -= mean;
        upDown -= mean;
        float center = rgb.g - mean;
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
        finalY = clamp(edgeSharpness * finalY, minY, maxY);
        float deltaY = clamp(finalY - center, -23.0 / 255.0, 23.0 / 255.0);
        rgb = clamp(rgb + deltaY, 0.0, 1.0);
    }
    color = vec4(rgb, 1.0);
}

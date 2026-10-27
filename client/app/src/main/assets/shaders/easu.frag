#version 300 es
#extension GL_OES_EGL_image_external_essl3 : require
// AMD FidelityFX FSR1, Copyright (c) 2021 Advanced Micro Devices, Inc.
// MIT license and upstream revision: ../NOTICE-FSR1.txt.
// FP32 EASU port: explicit OES taps replace gather4; native highp arithmetic replaces approximations.
precision highp float;
uniform highp samplerExternalOES source;
uniform mat4 textureTransform;
uniform vec4 con0;
uniform vec4 con1;
uniform vec4 con2;
uniform vec4 con3;
out vec4 color;

vec3 loadColor(vec2 p) {
    p = clamp(p, 0.5 * con1.xy, vec2(1.0) - 0.5 * con1.xy);
    return texture(source, (textureTransform * vec4(p, 0.0, 1.0)).xy).rgb;
}

float luma(vec3 c) {
    return c.b * 0.5 + (c.r * 0.5 + c.g);
}

void FsrEasuTapF(inout vec3 aC, inout float aW, vec2 off, vec2 dir,
                 vec2 len, float lob, float clp, vec3 c) {
    vec2 v = vec2(dot(off, dir), dot(off, vec2(-dir.y, dir.x))) * len;
    float d2 = min(dot(v, v), clp);
    float wB = (2.0 / 5.0) * d2 - 1.0;
    float wA = lob * d2 - 1.0;
    wB *= wB;
    wA *= wA;
    wB = (25.0 / 16.0) * wB - (25.0 / 16.0 - 1.0);
    float w = wB * wA;
    aC += c * w;
    aW += w;
}

void FsrEasuSetF(inout vec2 dir, inout float len, float w,
                 float lA, float lB, float lC, float lD, float lE) {
    vec2 gradient = vec2(lD - lB, lE - lA);
    // The upstream reciprocal approximation is finite even on flat areas.
    vec2 reversal = max(abs(vec2(lD, lE) - lC), abs(lC - vec2(lB, lA)));
    vec2 lengthXY = clamp(abs(gradient) / max(reversal, vec2(1.0e-8)), 0.0, 1.0);
    dir += gradient * w;
    len += dot(lengthXY, lengthXY) * w;
}

vec3 FsrEasuF(vec2 ip) {
    vec2 pp = ip * con0.xy + con0.zw;
    vec2 fp = floor(pp);
    pp -= fp;
    vec2 p0 = fp * con1.xy + con1.zw;
    vec2 p1 = p0 + con2.xy;
    vec2 p2 = p0 + con2.zw;
    vec2 p3 = p0 + con3.xy;
    vec2 halfTexel = 0.5 * con1.xy;
    vec3 b = loadColor(p0 + halfTexel * vec2(-1.0, 1.0));
    vec3 c = loadColor(p0 + halfTexel);
    vec3 i = loadColor(p1 + halfTexel * vec2(-1.0, 1.0));
    vec3 j = loadColor(p1 + halfTexel);
    vec3 f = loadColor(p1 + halfTexel * vec2(1.0, -1.0));
    vec3 e = loadColor(p1 - halfTexel);
    vec3 k = loadColor(p2 + halfTexel * vec2(-1.0, 1.0));
    vec3 l = loadColor(p2 + halfTexel);
    vec3 h = loadColor(p2 + halfTexel * vec2(1.0, -1.0));
    vec3 g = loadColor(p2 - halfTexel);
    vec3 o = loadColor(p3 + halfTexel * vec2(1.0, -1.0));
    vec3 n = loadColor(p3 - halfTexel);
    vec2 dir = vec2(0.0);
    float len = 0.0;
    FsrEasuSetF(dir, len, (1.0 - pp.x) * (1.0 - pp.y), luma(b), luma(e), luma(f), luma(g), luma(j));
    FsrEasuSetF(dir, len, pp.x * (1.0 - pp.y), luma(c), luma(f), luma(g), luma(h), luma(k));
    FsrEasuSetF(dir, len, (1.0 - pp.x) * pp.y, luma(f), luma(i), luma(j), luma(k), luma(n));
    FsrEasuSetF(dir, len, pp.x * pp.y, luma(g), luma(j), luma(k), luma(l), luma(o));
    float dirR = dot(dir, dir);
    dir = dirR < (1.0 / 32768.0) ? vec2(1.0, 0.0) : dir * inversesqrt(dirR);
    len *= 0.5;
    len *= len;
    float stretch = dot(dir, dir) / max(abs(dir.x), abs(dir.y));
    vec2 len2 = vec2(1.0 + (stretch - 1.0) * len, 1.0 - 0.5 * len);
    float lob = 0.5 + ((1.0 / 4.0 - 0.04) - 0.5) * len;
    float clp = 1.0 / lob;
    vec3 min4 = min(min(f, g), min(j, k));
    vec3 max4 = max(max(f, g), max(j, k));
    vec3 aC = vec3(0.0);
    float aW = 0.0;
    FsrEasuTapF(aC, aW, vec2( 0.0, -1.0) - pp, dir, len2, lob, clp, b);
    FsrEasuTapF(aC, aW, vec2( 1.0, -1.0) - pp, dir, len2, lob, clp, c);
    FsrEasuTapF(aC, aW, vec2(-1.0,  1.0) - pp, dir, len2, lob, clp, i);
    FsrEasuTapF(aC, aW, vec2( 0.0,  1.0) - pp, dir, len2, lob, clp, j);
    FsrEasuTapF(aC, aW, vec2( 0.0,  0.0) - pp, dir, len2, lob, clp, f);
    FsrEasuTapF(aC, aW, vec2(-1.0,  0.0) - pp, dir, len2, lob, clp, e);
    FsrEasuTapF(aC, aW, vec2( 1.0,  1.0) - pp, dir, len2, lob, clp, k);
    FsrEasuTapF(aC, aW, vec2( 2.0,  1.0) - pp, dir, len2, lob, clp, l);
    FsrEasuTapF(aC, aW, vec2( 2.0,  0.0) - pp, dir, len2, lob, clp, h);
    FsrEasuTapF(aC, aW, vec2( 1.0,  0.0) - pp, dir, len2, lob, clp, g);
    FsrEasuTapF(aC, aW, vec2( 1.0,  2.0) - pp, dir, len2, lob, clp, o);
    FsrEasuTapF(aC, aW, vec2( 0.0,  2.0) - pp, dir, len2, lob, clp, n);
    return clamp(aC / aW, min4, max4);
}

void main() {
    color = vec4(FsrEasuF(gl_FragCoord.xy - vec2(0.5)), 1.0);
}

// FSR1: Copyright (c) 2021 Advanced Micro Devices, Inc., MIT.
// SGSR1: Copyright (c) 2025 Qualcomm Innovation Center, Inc., BSD-3-Clause.
// Ported from app/src/main/assets/shaders; notices remain in app/src/main/assets.
#ifdef VULKAN
Texture2D<float4> source : register(t0, space2);
SamplerState sampleLinear : register(s0, space2);
cbuffer Picture : register(b0, space3) {
#else
Texture2D<float4> source : register(t0);
SamplerState sampleLinear : register(s0);
cbuffer Picture : register(b0) {
#endif
    float2 inputSize; float2 outputSize;
    float edgeSharpness; float sharpness; uint mode; float padding;
};
float4 vertex(uint id : SV_VertexID) : SV_Position {
    float2 p = float2((id << 1) & 2, id & 2);
    return float4(p * float2(2, -2) + float2(-1, 1), 0, 1);
}
float3 loadColor(float2 p) {
    return source.SampleLevel(sampleLinear, clamp(p, 0.5 / inputSize, 1.0 - 0.5 / inputSize), 0).rgb;
}
float luma(float3 c) {
    return c.b * 0.5 + (c.r * 0.5 + c.g);
}

void FsrEasuTapF(inout float3 aC, inout float aW, float2 off, float2 dir,
                 float2 len, float lob, float clp, float3 c) {
    float2 v = float2(dot(off, dir), dot(off, float2(-dir.y, dir.x))) * len;
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

void FsrEasuSetF(inout float2 dir, inout float len, float w,
                 float lA, float lB, float lC, float lD, float lE) {
    float2 gradient = float2(lD - lB, lE - lA);
    // The upstream reciprocal approximation is finite even on flat areas.
    float2 reversal = max(abs(float2(lD, lE) - lC), abs(lC - float2(lB, lA)));
    float2 lengthXY = clamp(abs(gradient) / max(reversal, float2(1.0e-8,1.0e-8)), 0.0, 1.0);
    dir += gradient * w;
    len += dot(lengthXY, lengthXY) * w;
}

float3 FsrEasuF(float2 ip) {
    float4 con0 = float4(inputSize / outputSize, 0.5 * inputSize / outputSize - 0.5);
    float4 con1 = float4(1.0 / inputSize, float2(1.0, -1.0) / inputSize);
    float4 con2 = float4(float2(-1.0, 2.0) / inputSize, float2(1.0, 2.0) / inputSize);
    float4 con3 = float4(0.0, 4.0 / inputSize.y, 0.0, 0.0);
    float2 pp = ip * con0.xy + con0.zw;
    float2 fp = floor(pp);
    pp -= fp;
    float2 p0 = fp * con1.xy + con1.zw;
    float2 p1 = p0 + con2.xy;
    float2 p2 = p0 + con2.zw;
    float2 p3 = p0 + con3.xy;
    float2 halfTexel = 0.5 * con1.xy;
    float3 b = loadColor(p0 + halfTexel * float2(-1.0, 1.0));
    float3 c = loadColor(p0 + halfTexel);
    float3 i = loadColor(p1 + halfTexel * float2(-1.0, 1.0));
    float3 j = loadColor(p1 + halfTexel);
    float3 f = loadColor(p1 + halfTexel * float2(1.0, -1.0));
    float3 e = loadColor(p1 - halfTexel);
    float3 k = loadColor(p2 + halfTexel * float2(-1.0, 1.0));
    float3 l = loadColor(p2 + halfTexel);
    float3 h = loadColor(p2 + halfTexel * float2(1.0, -1.0));
    float3 g = loadColor(p2 - halfTexel);
    float3 o = loadColor(p3 + halfTexel * float2(1.0, -1.0));
    float3 n = loadColor(p3 - halfTexel);
    float2 dir = float2(0.0,0.0);
    float len = 0.0;
    FsrEasuSetF(dir, len, (1.0 - pp.x) * (1.0 - pp.y), luma(b), luma(e), luma(f), luma(g), luma(j));
    FsrEasuSetF(dir, len, pp.x * (1.0 - pp.y), luma(c), luma(f), luma(g), luma(h), luma(k));
    FsrEasuSetF(dir, len, (1.0 - pp.x) * pp.y, luma(f), luma(i), luma(j), luma(k), luma(n));
    FsrEasuSetF(dir, len, pp.x * pp.y, luma(g), luma(j), luma(k), luma(l), luma(o));
    float dirR = dot(dir, dir);
    dir = dirR < (1.0 / 32768.0) ? float2(1.0, 0.0) : dir * rsqrt(dirR);
    len *= 0.5;
    len *= len;
    float stretch = dot(dir, dir) / max(abs(dir.x), abs(dir.y));
    float2 len2 = float2(1.0 + (stretch - 1.0) * len, 1.0 - 0.5 * len);
    float lob = 0.5 + ((1.0 / 4.0 - 0.04) - 0.5) * len;
    float clp = 1.0 / lob;
    float3 min4 = min(min(f, g), min(j, k));
    float3 max4 = max(max(f, g), max(j, k));
    float3 aC = float3(0.0,0.0,0.0);
    float aW = 0.0;
    FsrEasuTapF(aC, aW, float2( 0.0, -1.0) - pp, dir, len2, lob, clp, b);
    FsrEasuTapF(aC, aW, float2( 1.0, -1.0) - pp, dir, len2, lob, clp, c);
    FsrEasuTapF(aC, aW, float2(-1.0,  1.0) - pp, dir, len2, lob, clp, i);
    FsrEasuTapF(aC, aW, float2( 0.0,  1.0) - pp, dir, len2, lob, clp, j);
    FsrEasuTapF(aC, aW, float2( 0.0,  0.0) - pp, dir, len2, lob, clp, f);
    FsrEasuTapF(aC, aW, float2(-1.0,  0.0) - pp, dir, len2, lob, clp, e);
    FsrEasuTapF(aC, aW, float2( 1.0,  1.0) - pp, dir, len2, lob, clp, k);
    FsrEasuTapF(aC, aW, float2( 2.0,  1.0) - pp, dir, len2, lob, clp, l);
    FsrEasuTapF(aC, aW, float2( 2.0,  0.0) - pp, dir, len2, lob, clp, h);
    FsrEasuTapF(aC, aW, float2( 1.0,  0.0) - pp, dir, len2, lob, clp, g);
    FsrEasuTapF(aC, aW, float2( 1.0,  2.0) - pp, dir, len2, lob, clp, o);
    FsrEasuTapF(aC, aW, float2( 0.0,  2.0) - pp, dir, len2, lob, clp, n);
    return clamp(aC / aW, min4, max4);
}

float3 FsrRcasLoadF(int2 p) {
    return source.Load(int3(clamp(p, int2(0, 0), int2(inputSize) - 1), 0)).rgb;
}

float3 FsrRcasF(int2 p) {
    float3 b = FsrRcasLoadF(p + int2(0, -1));
    float3 d = FsrRcasLoadF(p + int2(-1, 0));
    float3 e = FsrRcasLoadF(p);
    float3 f = FsrRcasLoadF(p + int2(1, 0));
    float3 h = FsrRcasLoadF(p + int2(0, 1));
    float3 mn4 = min(min(b, d), min(f, h));
    float3 mx4 = max(max(b, d), max(f, h));
    // Keep black/white plateaus finite on GLES implementations that propagate 0/0.
    float3 hitMin = min(mn4, e) / max(4.0 * mx4, float3(1.0e-8,1.0e-8,1.0e-8));
    float3 hitMax = (1.0 - max(mx4, e)) / min(4.0 * mn4 - 4.0, float3(-1.0e-8,-1.0e-8,-1.0e-8));
    float3 lobes = max(-hitMin, hitMax);
    float lobe = max(-(0.25 - 1.0 / 16.0), min(max(max(lobes.r, lobes.g), lobes.b), 0.0)) * sharpness;
    return (lobe * (b + d + f + h) + e) / (4.0 * lobe + 1.0);
}

float loadGreen(float2 pixel) {
    return loadColor((pixel + 0.5) * (1.0 / inputSize)).g;
}

float fastLanczos2(float x) {
    float wA = x - 4.0;
    float wB = x * wA - wA;
    wA *= wA;
    return wB * wA;
}

float2 weightY(float dx, float dy, float c, float std) {
    float x = (dx * dx + dy * dy) * 0.55 + clamp(abs(c) * std, 0.0, 1.0);
    float w = fastLanczos2(x);
    return float2(w, w * c);
}

float3 Sgsr(float2 uv) {
    float4 viewportInfo = float4(1.0 / inputSize, inputSize);
    float3 rgb = loadColor(uv);
    float2 imgCoord = uv * viewportInfo.zw + float2(-0.5, 0.5);
    float2 pixel = floor(imgCoord);
    float2 pl = imgCoord - pixel;

    // GLSL gather order is upper-left, upper-right, lower-right, lower-left.
    float4 left = float4(loadGreen(pixel + float2(-1.0, 0.0)), loadGreen(pixel),
                     loadGreen(pixel + float2(0.0, -1.0)), loadGreen(pixel + float2(-1.0, -1.0)));
    float edgeVote = abs(left.z - left.y) + abs(rgb.g - left.y) + abs(rgb.g - left.z);
    if (edgeVote > 8.0 / 255.0) {
        float4 right = float4(loadGreen(pixel + float2(1.0, 0.0)), loadGreen(pixel + float2(2.0, 0.0)),
                         loadGreen(pixel + float2(2.0, -1.0)), loadGreen(pixel + float2(1.0, -1.0)));
        float4 upDown = float4(loadGreen(pixel + float2(0.0, -2.0)), loadGreen(pixel + float2(1.0, -2.0)),
                          loadGreen(pixel + float2(1.0, 1.0)), loadGreen(pixel + float2(0.0, 1.0)));
        float mean = (left.y + left.z + right.x + right.w) * 0.25;
        left -= mean;
        right -= mean;
        upDown -= mean;
        float center = rgb.g - mean;
        float sum = dot(abs(left) + abs(right) + abs(upDown), float4(1.0,1.0,1.0,1.0));
        float std = 2.181818 / max(sum, 1.0e-8);

        float2 aWY = weightY(pl.x, pl.y + 1.0, upDown.x, std);
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
    return rgb;
}

float4 fragment(float4 position : SV_Position) : SV_Target {
    float2 uv = position.xy / outputSize;
    if (mode == 1) return float4(FsrEasuF(position.xy - 0.5), 1);
    if (mode == 2) return float4(FsrRcasF(int2(position.xy)), 1);
    // SGSR's gather positions use bottom-left image coordinates.
    return float4(Sgsr(uv), 1);
}

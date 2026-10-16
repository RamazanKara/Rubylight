
Texture2D<float4> source : register(t0);
cbuffer Config : register(b0) {
    uint2 sourceSize; uint2 targetSize;
    uint pixel; uint hdr; uint colorMatrix; uint fullRange;
    uint tenBit; float sdrWhiteScale; float hdrScale; uint padding;
    uint4 reserved[2];
};
float3 srgb_linear(float3 rgb) { return lerp(pow((rgb + 0.055) / 1.055, 2.4), rgb / 12.92, step(rgb, 0.04045)); }
float3 gamma(float3 rgb) { return lerp(1.055 * pow(max(rgb, 0), 1.0 / 2.4) - 0.055, rgb * 12.92, step(rgb, 0.0031308)); }
// BT.2020's 10-bit SDR OETF (transfer_characteristics = 14), not sRGB or PQ.
float3 bt2020_sdr(float3 light) {
    light = saturate(light);
    return lerp(1.099 * pow(light, 0.45) - 0.099, 4.5 * light, step(light, 0.018));
}
float3 gamut2020(float3 rgb) {
    return float3(dot(rgb, float3(0.627404, 0.329283, 0.043313)),
                  dot(rgb, float3(0.069097, 0.919540, 0.011362)),
                  dot(rgb, float3(0.016391, 0.088013, 0.895595)));
}
float3 pq(float3 luminance) {
    float3 power = pow(clamp(luminance, 0, 1), 2610.0 / 16384.0);
    return pow((3424.0 / 4096.0 + 2413.0 / 128.0 * power) /
               (1.0 + 2392.0 / 128.0 * power), 2523.0 / 32.0);
}
float3 depq(float3 rgb) {
    float3 p = pow(clamp(rgb, 0, 1), 32.0 / 2523.0);
    return pow(max(p - 3424.0 / 4096.0, 0) / max(2413.0 / 128.0 - 2392.0 / 128.0 * p, 0.000001), 16384.0 / 2610.0);
}
float3 load(int2 p) {
    p = clamp(p, int2(0, 0), int2(sourceSize) - 1);
    float3 rgb = source.Load(int3(p, 0)).rgb;
    if (hdr != 0 && pixel == 0) {
        rgb = srgb_linear(rgb);
        rgb *= sdrWhiteScale;
    }
    if (hdr != 0 && pixel == 1) rgb *= hdrScale;
    // Normalize scRGB to SDR white. A BT.2020 stream keeps linear, extended
    // Rec.709 values until gamut mapping; clipping to sRGB here would lose
    // wide-gamut colors that are still inside BT.2020.
    if (hdr == 0 && pixel == 1) {
        rgb /= sdrWhiteScale;
        if (colorMatrix != 2) rgb = gamma(saturate(rgb));
    }
    return rgb;
}
// The source keeps its aspect ratio, centred in the stream with black bars
// (a 32:9 desktop streamed to a 16:10 tablet). Sizes within a pixel of the
// stream's shape still fill it, so rounding never adds a one-pixel line.
bool content(float2 target, out float2 p) {
    float2 ratio = float2(targetSize) / float2(sourceSize);
    float scale = min(ratio.x, ratio.y);
    float2 size = float2(sourceSize) * scale;
    if (all(abs(size - float2(targetSize)) < 1)) {
        p = (target + 0.5) / ratio - 0.5;
        return true;
    }
    float2 at = target + 0.5 - (float2(targetSize) - size) * 0.5;
    p = at / scale - 0.5;
    return all(at >= 0) && all(at < size);
}
float3 bilinear(float2 p) {
    int2 at = int2(floor(p));
    float2 f = frac(p);
    return lerp(lerp(load(at), load(at + int2(1, 0)), f.x),
                lerp(load(at + int2(0, 1)), load(at + int2(1, 1)), f.x), f.y);
}
// Integrate source-pixel coverage when reducing an image. A fixed bilinear
// footprint skips thin features at fractional and >2x reductions, so they
// shimmer as the source moves. Native size and enlargement keep their old
// paths. An aligned 2x reduction already has the exact four-sample average.
float3 resize(float2 p, float2 footprint) {
    float2 left = p + 0.5 - footprint * 0.5;
    if (any(footprint < 1) || all(footprint <= 1) || any(footprint > 8) ||
        (all(footprint == 2) && all(frac(left) == 0))) return bilinear(p);
    float2 right = left + footprint;
    int2 first = int2(floor(left));
    int2 count = int2(ceil(right)) - first;
    float3 rgb = 0;
    // At most nine source pixels intersect a footprint of up to eight.
    // Keep a hard bound even for extreme stream/source size ratios; those
    // retain the previous bilinear path instead of an unbounded shader loop.
    [loop] for (int y = 0; y < 9; ++y) {
        if (y >= count.y) break;
        float wy = max(0, min(right.y, first.y + y + 1) - max(left.y, first.y + y));
        [loop] for (int x = 0; x < 9; ++x) {
            if (x >= count.x) break;
            float wx = max(0, min(right.x, first.x + x + 1) - max(left.x, first.x + x));
            rgb += load(first + int2(x, y)) * (wx * wy);
        }
    }
    return rgb / (footprint.x * footprint.y);
}
float3 nonlinear(float2 target) {
    float3 rgb;
    if (all(sourceSize == targetSize)) {
        rgb = load(int2(target));
    } else if (all(sourceSize == targetSize * 2)) {
        // A uniform shape test avoids per-pixel coverage/control-flow work on
        // the common 4K -> 1080p path. Its four samples are already exact.
        rgb = bilinear(target * 2 + 0.5);
    } else if (all(sourceSize == targetSize * 3)) {
        // The other common integer reduction (4K -> 720p) covers nine whole
        // source pixels. Fixed unweighted loops keep the row-order sum without
        // duplicating the cursor/HDR load logic into every sample in bytecode.
        int2 at = int2(target) * 3;
        rgb = 0;
        [loop] for (int y = 0; y < 3; ++y) {
            [loop] for (int x = 0; x < 3; ++x) {
                rgb += load(at + int2(x, y));
            }
        }
        rgb /= 9;
    } else {
        float2 p;
        if (!content(target, p)) return float3(0, 0, 0);
        float2 ratio = float2(targetSize) / float2(sourceSize);
        float scale = min(ratio.x, ratio.y);
        float2 size = float2(sourceSize) * scale;
        float2 footprint = all(abs(size - float2(targetSize)) < 1) ? 1 / ratio : 1 / scale;
        rgb = resize(p, footprint);
    }
    if (hdr == 0) {
        // SDR desktop RGB uses sRGB transfer and Rec.709 primaries. Applying
        // only the BT.2020 YUV coefficients would relabel those primaries.
        return colorMatrix == 2 ? bt2020_sdr(gamut2020(pixel == 1 ? rgb : srgb_linear(rgb))) : rgb;
    }
    if (pixel == 2) return rgb;
    return pq(gamut2020(rgb) / 125.0);
}
float3 weights() { return colorMatrix == 2 ? float3(0.2627, 0.6780, 0.0593) : colorMatrix == 0 ? float3(0.299, 0.587, 0.114) : float3(0.2126, 0.7152, 0.0722); }
// The luma code of a pixel, and the chroma code of filtered nonlinear RGB.
float lumaCode(float3 rgb) {
    float y = dot(rgb, weights());
    return tenBit != 0 ? floor(clamp((fullRange != 0 ? 1023 * y : 64 + 876 * y), 0, 1023) + 0.5) * 64 / 65535.0
                       : floor(clamp((fullRange != 0 ? 255 * y : 16 + 219 * y), 0, 255) + 0.5) / 255.0;
}
float2 chromaCode(float3 rgb) {
    float3 k = weights();
    float y = dot(rgb, k);
    float2 uv = float2((rgb.b - y) / (2 * (1 - k.b)), (rgb.r - y) / (2 * (1 - k.r)));
    return tenBit != 0 ? floor(clamp(512 + (fullRange != 0 ? 1023 : 896) * uv, 0, 1023) + 0.5) * 64 / 65535.0
                       : floor(clamp(128 + (fullRange != 0 ? 255 : 224) * uv, 0, 255) + 0.5) / 255.0;
}
// Standard-codec 4:2:0 is horizontally co-sited with the left luma sample
// and vertically centred. A symmetric [1,2,1] x [1,1] filter preserves that
// position while suppressing aliasing. Clamp in target coordinates so the
// left edge also works with resizing and letterboxing.
float3 leftChroma(float2 at, float3 c00, float3 c10, float3 c01, float3 c11) {
    float2 left = float2(max(at.x - 1, 0), at.y);
    return (nonlinear(left) + nonlinear(left + float2(0, 1)) +
            2 * (c00 + c01) + c10 + c11) * 0.125;
}
// NV12 or P010 on a compute queue, which keeps running while a game fills the
// graphics queue. The two views address the luma and chroma planes.
RWTexture2D<float> lumaPlane : register(u0);
RWTexture2D<float2> chromaPlane : register(u1);
// One thread per 2x2 block: reuse its four luma samples for chroma, plus
// the two samples to the left needed for the co-sited horizontal filter.
[numthreads(8, 8, 1)]
void yuv420_cs(uint3 id : SV_DispatchThreadID) {
    if (any(id.xy >= targetSize / 2)) return;
    uint2 at = id.xy * 2;
    float3 c00 = nonlinear(float2(at));
    float3 c10 = nonlinear(float2(at + uint2(1, 0)));
    float3 c01 = nonlinear(float2(at + uint2(0, 1)));
    float3 c11 = nonlinear(float2(at + uint2(1, 1)));
    lumaPlane[at] = lumaCode(c00);
    lumaPlane[at + uint2(1, 0)] = lumaCode(c10);
    lumaPlane[at + uint2(0, 1)] = lumaCode(c01);
    lumaPlane[at + uint2(1, 1)] = lumaCode(c11);
    chromaPlane[id.xy] = chromaCode(leftChroma(float2(at), c00, c10, c01, c11));
}
// Version 1 is a permutation of full-resolution quantized Y/U/V. Padding
// extends the logical edge before conversion, without a chroma filter.
uint2 packedSize() { return (targetSize + uint2(63, 7)) & ~uint2(63, 7); }
float3 packedRgb(uint2 at) { return nonlinear(float2(min(at, targetSize - 1))); }
// The extra chroma regions are signed half-differences in version 2. Keeping
// their full range avoids modular wraparound when the codec quantizes them.
float2 packedDetail(float2 value, float2 base) {
    float unit = tenBit != 0 ? 65535.0 / 64 : 255;
    float maximum = tenBit != 0 ? 1023 : 255;
    float center = tenBit != 0 ? 512 : 128;
    return clamp(floor((round(value * unit) - round(base * unit)) * 0.5 + center + 0.5), 0, maximum) / unit;
}
[numthreads(8, 8, 1)]
void chroma_pack_cs(uint3 id : SV_DispatchThreadID) {
    uint2 size = packedSize();
    if (any(id.xy >= uint2(size.x / 2, size.y))) return;
    uint2 at = uint2(id.x * 2, id.y);
    float3 even = packedRgb(at);
    float3 odd = packedRgb(at + uint2(1, 0));
    if (padding == 2) {
        // Each thread owns one complete 2x2 block and reuses the anchor for
        // all three enhancement samples; the lower half of the dispatch exits.
        if (id.y >= size.y / 2) return;
        at.y = id.y * 2;
        even = packedRgb(at);
        odd = packedRgb(at + uint2(1, 0));
        float3 bottomLeft = packedRgb(at + uint2(0, 1));
        float3 bottomRight = packedRgb(at + uint2(1, 1));
        float2 base = chromaCode(even);
        float2 topDetail = packedDetail(chromaCode(odd), base);
        float2 bottomDetail = packedDetail(chromaCode(bottomRight), base);
        lumaPlane[at] = lumaCode(even);
        lumaPlane[at + uint2(1, 0)] = lumaCode(odd);
        lumaPlane[at + uint2(0, 1)] = lumaCode(bottomLeft);
        lumaPlane[at + uint2(1, 1)] = lumaCode(bottomRight);
        lumaPlane[uint2(id.x, size.y + at.y)] = topDetail.x;
        lumaPlane[uint2(size.x / 2 + id.x, size.y + at.y)] = topDetail.y;
        lumaPlane[uint2(id.x, size.y + at.y + 1)] = bottomDetail.x;
        lumaPlane[uint2(size.x / 2 + id.x, size.y + at.y + 1)] = bottomDetail.y;
        chromaPlane[id.xy] = base;
        chromaPlane[uint2(id.x, size.y / 2 + id.y)] = packedDetail(chromaCode(bottomLeft), base);
        return;
    }
    float2 uv = chromaCode(odd);
    lumaPlane[at] = lumaCode(even);
    lumaPlane[at + uint2(1, 0)] = lumaCode(odd);
    lumaPlane[uint2(id.x, size.y + id.y)] = uv.x;
    lumaPlane[uint2(size.x / 2 + id.x, size.y + id.y)] = uv.y;
    chromaPlane[id.xy] = chromaCode(even);
}
// PyroWave's planes hold codes over their full range (code / 1023 or 255),
// one plane each.
float pyroLuma(float3 rgb) {
    float y = dot(rgb, weights());
    float maximum = tenBit != 0 ? 1023 : 255;
    float code = tenBit != 0 ? (fullRange != 0 ? 1023*y : 64+876*y) : (fullRange != 0 ? 255*y : 16+219*y);
    return floor(clamp(code,0,maximum)+0.5)/maximum;
}
float2 pyroChroma(float3 rgb) {
    float3 k = weights(); float y = dot(rgb,k);
    float2 uv = float2((rgb.b-y)/(2*(1-k.b)),(rgb.r-y)/(2*(1-k.r)));
    float maximum = tenBit != 0 ? 1023 : 255;
    float2 code = tenBit != 0 ? 512+(fullRange != 0 ? 1023 : 896)*uv : 128+(fullRange != 0 ? 255 : 224)*uv;
    return floor(clamp(code,0,maximum)+0.5)/maximum;
}
// PyroWave's decoder explicitly uses centre-sited 4:2:0 (pyrowave.h).
// Keep its 2x2 average separate from standard codecs' left-sited filter.
float2 pyroBlock(float3 c00, float3 c10, float3 c01, float3 c11) {
    return pyroChroma((c00 + c10 + c01 + c11) * 0.25);
}
// PyroWave's three planes on the compute queue, in one pass over 2x2 blocks,
// beside a game that fills the graphics queue.
RWTexture2D<float> pyroY : register(u2);
RWTexture2D<float> pyroU : register(u3);
RWTexture2D<float> pyroV : register(u4);
[numthreads(8, 8, 1)]
void pyro_cs(uint3 id : SV_DispatchThreadID) {
    if (any(id.xy >= targetSize / 2)) return;
    uint2 at = id.xy * 2;
    float3 c00 = nonlinear(float2(at));
    float3 c10 = nonlinear(float2(at + uint2(1, 0)));
    float3 c01 = nonlinear(float2(at + uint2(0, 1)));
    float3 c11 = nonlinear(float2(at + uint2(1, 1)));
    pyroY[at] = pyroLuma(c00);
    pyroY[at + uint2(1, 0)] = pyroLuma(c10);
    pyroY[at + uint2(0, 1)] = pyroLuma(c01);
    pyroY[at + uint2(1, 1)] = pyroLuma(c11);
    if (padding != 0) {
        float2 c = pyroChroma(c00); pyroU[at] = c.x; pyroV[at] = c.y;
        c = pyroChroma(c10); pyroU[at + uint2(1, 0)] = c.x; pyroV[at + uint2(1, 0)] = c.y;
        c = pyroChroma(c01); pyroU[at + uint2(0, 1)] = c.x; pyroV[at + uint2(0, 1)] = c.y;
        c = pyroChroma(c11); pyroU[at + uint2(1, 1)] = c.x; pyroV[at + uint2(1, 1)] = c.y;
    } else {
        float2 c = pyroBlock(c00, c10, c01, c11);
        pyroU[id.xy] = c.x;
        pyroV[id.xy] = c.y;
    }
}

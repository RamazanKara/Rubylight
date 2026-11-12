// CDF 9/7 lifting follows Themaister/pyrowave a0d2e71 (MIT, Hans-Kristian Arntzen).
#ifdef VULKAN
Texture2D<float4> source : register(t0, space2);
SamplerState sampleLinear : register(s0, space2);
cbuffer Picture : register(b0, space3) {
#else
Texture2D<float4> source : register(t0);
SamplerState sampleLinear : register(s0);
cbuffer Picture : register(b0) {
#endif
    uint width; uint height; uint paddedWidth; uint paddedHeight;
    uint level; uint operation; uint flags; float sdrWhite;
#ifdef VULKAN
    // Flag 512, the planes entry only (planes_gpu.rs `draw_direct`): the target rectangle the
    // picture fills (x, y, w, h in target pixels), the picture's visible size, and what SDL's own
    // draw of the picture texture would do after the conversion: its tone map (factors 1 and 2,
    // both 0 for none) and its colour scale. The other draws leave these out.
    float4 destination; float4 picture;
    float outputScale; float tonemap1; float tonemap2; float unused;
    // Flag 8192 (with 4096): the gamma 2.2 range, minNits to the peak (invRange = 1 / (peak - minNits)).
    float minNits; float invRange; float2 unusedEncoding;
    // Flag 512: Frame Warp (stream/frame_warp.rs), the shift in fractions of the picture, then
    // the HUD mask's size in cells (0 for none). All zero draws the picture where it is. Then
    // its lens terms (0, 0, 1, 1 for a flat picture).
    float4 warp;
    float4 warpLens;
#endif
};
float4 vertex(uint id : SV_VertexID) : SV_Position {
    float2 p = float2((id << 1) & 2, id & 2);
    return float4(p * float2(2, -2) + float2(-1, 1), 0, 1);
}
float3 coefficient(int i, int across, int length) {
    i = abs(i);
    if (i >= length) i = 2 * length - 2 - i;
    int index = i / 2 + ((i & 1) != 0 ? length / 2 : 0);
    int2 p = operation == 0 ? int2(across, index) : int2(index, across);
    return source.Load(int3(p, 0)).rgb;
}
float3 inverse(int i, int a, int n) {
    // Expanded lifting steps share symmetric taps instead of repeatedly loading neighbors.
    if ((i & 1) == 0) {
        return 1.11508705 * coefficient(i, a, n)
            - 0.26686413 * (coefficient(i - 1, a, n) + coefficient(i + 1, a, n))
            - 0.057543526 * (coefficient(i - 2, a, n) + coefficient(i + 2, a, n))
            + 0.016864119 * (coefficient(i - 3, a, n) + coefficient(i + 3, a, n));
    }
    return 0.60294904 * coefficient(i, a, n)
        + 0.59127169 * (coefficient(i - 1, a, n) + coefficient(i + 1, a, n))
        - 0.078223256 * (coefficient(i - 2, a, n) + coefficient(i + 2, a, n))
        - 0.091271760 * (coefficient(i - 3, a, n) + coefficient(i + 3, a, n))
        + 0.026748758 * (coefficient(i - 4, a, n) + coefficient(i + 4, a, n));
}
float3 linearRgb(float3 samples) {
    bool ten = (flags & 16) != 0;
    bool full = (flags & 1) != 0;
    bool hdr = (flags & 2) != 0;
    uint colorMatrix = (flags >> 2) & 3;
    float maximum = ten ? 1023 : 255;
    float factor = ten ? 4 : 1;
    float3 code = saturate(samples + 0.5) * maximum;
    float y = (code.x - (full ? 0 : 16 * factor)) / (full ? maximum : 219 * factor);
    float2 uv = (code.yz - 128 * factor) / (full ? maximum : 224 * factor);
    float3 k = hdr || colorMatrix == 2 ? float3(0.2627, 0.6780, 0.0593) :
               colorMatrix == 0 ? float3(0.299, 0.587, 0.114) : float3(0.2126, 0.7152, 0.0722);
    float r = y + 2 * (1 - k.r) * uv.y;
    float b = y + 2 * (1 - k.b) * uv.x;
    float3 rgb = float3(r, (y - k.r * r - k.b * b) / k.g, b);
    if (!hdr) {
        rgb = saturate(rgb);
        return lerp(pow((rgb + 0.055) / 1.055, 2.4), rgb / 12.92, step(rgb, 0.04045));
    }
    if ((flags & 128) != 0) {
        // HLG (the planes entry only, for planar and NV12/P010 pictures): BT.2100's inverse OETF,
        // then the OOTF of the display it is drawn for, whose peak in nits and system gamma come
        // as floats in `level` and `operation` (hlg.rs `hlg_display`), in units of 10000 nits
        // like PQ below.
        float3 scene = saturate(rgb);
        scene = scene <= 0.5 ? scene * scene / 3 : (exp((scene - 0.55991073) / 0.17883277) + 0.28466892) / 12;
        float luma = dot(scene, float3(0.2627, 0.6780, 0.0593));
        rgb = (luma > 0 ? asfloat(level) / 10000 * pow(luma, asfloat(operation) - 1) * scene : 0) * (125 / sdrWhite);
    } else {
        float3 p = pow(saturate(rgb), 32.0 / 2523.0);
        rgb = pow(max(p - 3424.0 / 4096.0, 0) / max(2413.0 / 128.0 - 2392.0 / 128.0 * p, 0.000001), 16384.0 / 2610.0) * (125 / sdrWhite);
    }
    return float3(dot(rgb, float3(1.660491, -0.587641, -0.072850)),
                  dot(rgb, float3(-0.124550, 1.132900, -0.008349)),
                  dot(rgb, float3(-0.018151, -0.100579, 1.118730)));
}
#ifdef VULKAN
float3 storedHalf(float3 x) { return x; }
#else
// The wavelet passes keep their samples in half-float targets, ten stores per frame. Direct3D 11
// on the RX 7900 XT cuts the extra mantissa bits off instead of rounding (measured: the dark
// parts of a picture came out 0.34 code too bright, white parts 0.1 too dark), and ten cuts add
// up. Rounding to the 11 significant bits of a half here (Veltkamp's split) makes the cut exact.
float3 storedHalf(float3 x) {
    precise float3 c = x * 8193.0;
    precise float3 high = c - (c - x);
    return high;
}
#endif
float4 fragment(float4 position : SV_Position) : SV_Target {
    int2 p = int2(position.xy);
    float3 original = source.Load(int3(p, 0)).rgb;
    if (operation < 2) {
        int2 size = int2(paddedWidth, paddedHeight) >> level;
        if (any(p >= size)) return float4(original, 1);
        float3 result = operation == 0 ? inverse(p.y, p.x, size.y) : inverse(p.x, p.y, size.x);
        if ((flags & 32) == 0 && level == 0) result.gb = original.gb;
        return float4(storedHalf(result), 1);
    }
    if ((flags & 32) == 0) {
        float2 uv = clamp((position.xy * 0.5), 0.5, float2(width, height) * 0.5 - 0.5) / float2(paddedWidth, paddedHeight);
        original.gb = source.SampleLevel(sampleLinear, uv, 0).gb;
    }
    return float4(linearRgb(original), 1);
}
// The GPU decoder's planes: R8 or R16 UNORM textures holding code / maximum, converted to the
// same linear RGB as the CPU path (the planes are final, so no wavelet passes are left). The
// Vulkan build is a module of its own (pyrowave-planes.spv, see docs/pyrowave-decode.md): the planes take
// SDL's sampler slots 0 to 2 and glslang gives each texture its own sampler. Flag 64: R16 planes
// holding FFmpeg's 10-bit samples in their low bits (code / 65535), as HEVC and AV1 decode them.
// Flag 256, Vulkan build only: Vulkan video's planes (`vulkan_video::Planes`), Cb and Cr
// interleaved in planeCb (R8G8 or R16G16, from NV12, P010, NV24 or P410), and with flag 16 the
// 10-bit samples in the high bits of each word (code * 64 / 65535). Flag 2048, Vulkan build only:
// a mapped 4:4:4 VAAPI picture copied whole into planeCb, R8G8B8A8 holding Cr, Cb, Y (XYUV) or,
// with flag 16, A2B10G10R10 holding Cb, Y, Cr (Y410 copied bit for bit).
#ifdef VULKAN
Texture2D<float> planeY : register(t0, space2);
Texture2D<float4> planeCb : register(t1, space2);
Texture2D<float> planeCr : register(t2, space2);
// Frame Warp's HUD mask, flag 512 only: byte x / 8 of row y holds cell x in bit x % 8.
Texture2D<float> hudMask : register(t3, space2);
#define CB(cb) ((cb).x)
#define CR(cb, cr) (((flags & 256) != 0) ? (cb).y : (cr))
#define HIGH_BITS if ((flags & 272) == 272) samples *= 65535.0 / 65472.0;
#define INTERLEAVED(t) (((flags & 16) != 0) ? (t).grb : (t).bgr)
#else
Texture2D<float> planeY : register(t1);
Texture2D<float> planeCb : register(t2);
Texture2D<float> planeCr : register(t3);
#define CB(cb) cb
#define CR(cb, cr) cr
#define HIGH_BITS
#endif
#ifdef VULKAN
static const float3x3 bt709to2020 = { 0.627404, 0.329283, 0.043313, 0.069097, 0.919541, 0.011362, 0.016391, 0.088013, 0.895595 };
static const float3x3 bt2020to709 = { 1.660496, -0.587656, -0.072840, -0.124547, 1.132895, -0.008348, -0.018154, -0.100597, 1.118751 };
// SDL's sRGBfromLinear (texture_advanced.frag.hlsl), for a target that is not linear.
float3 srgbFromLinear(float3 v) {
    return lerp(pow(abs(v), 1.0 / 2.4) * 1.055 - 0.055, v * 12.92, float3(v <= 0.0031308));
}
// The PQ present pass's encode (pq_present.hlsl, its matrix and curve exactly): scRGB (BT.709,
// 1.0 = 80 nits) to BT.2020 PQ (SMPTE ST 2084).
float3 pqFromScrgb(float3 linear709) {
    const float3x3 to2020 = float3x3(
        0.6274040, 0.3292820, 0.0433136,
        0.0690970, 0.9195400, 0.0113612,
        0.0163916, 0.0880132, 0.8955950);
    float3 nits = max(mul(to2020, linear709), 0.0) * 80.0;
    const float m1 = 2610.0 / 16384.0;
    const float m2 = 2523.0 / 4096.0 * 128.0;
    const float c1 = 3424.0 / 4096.0;
    const float c2 = 2413.0 / 4096.0 * 32.0;
    const float c3 = 2392.0 / 4096.0 * 32.0;
    float3 y = pow(saturate(nits / 10000.0), m1);
    return pow((c1 + c2 * y) / (1.0 + c3 * y), m2);
}
// A 4x4 ordered dither in (0, 1), the same as scanout_present.hlsl's.
static const float bayer[16] = { 0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5 };
float orderedDither(float2 position) {
    uint2 p = uint2(position) & 3;
    return (bayer[p.y * 4 + p.x] + 0.5) / 16.0;
}
// Flag 8192: the scanout present pass's gamma 2.2 encode (scanout_present.hlsl, exactly): scRGB
// to BT.2020 nits, then gamma 2.2 over minNits..peak with half a 10-bit step of ordered dither.
float3 gamma22FromScrgb(float3 linear709, float2 position) {
    const float3x3 to2020 = float3x3(
        0.6274040, 0.3292820, 0.0433136,
        0.0690970, 0.9195400, 0.0113612,
        0.0163916, 0.0880132, 0.8955950);
    float3 nits = max(mul(to2020, linear709), 0.0) * 80.0;
    float3 encoded = pow(saturate((nits - minNits) * invRange), 1.0 / 2.2);
    return saturate(encoded + (orderedDither(position) - 0.5) / 1023.0);
}
// Frame Warp, for flag 512.
#include "frame_warp.hlsl"
// Flag 512: the planes scaled straight into the target rectangle (bilinear, as SDL scales the
// picture texture), then SDL's tone map (TONEMAP_CHROME of its texture_advanced shader), colour
// scale and, with flag 1024 (an sRGB target, not scRGB), its sRGB encoding and clamp, so the
// result is what drawing the converted texture with SDL gives, in one pass. With flag 4096 instead
// of 1024 (an HDR10 swapchain image taken in a video-only frame, window-target.patch), the scRGB
// result is PQ-encoded as the PQ present pass would encode the back buffer. Frame Warp moves the
// point sampled (`frameWarp`, in the picture's pixels) and darkens its edge fill by `fade`: the
// output, or on a PQ target the light it encodes.
float4 planesDirect(float2 position) {
    float2 visible = picture.xy;
    float fade;
    float2 at = clamp(frameWarp((position - destination.xy) * visible / destination.zw, visible, fade), 0.5, visible - 0.5);
    float3 samples;
    uint2 size;
    planeCb.GetDimensions(size.x, size.y);
    if ((flags & 2048) != 0) {
        samples = INTERLEAVED(planeCb.SampleLevel(sampleLinear, at / float2(size), 0));
    } else {
        uint2 lumaSize;
        planeY.GetDimensions(lumaSize.x, lumaSize.y);
        samples.x = planeY.SampleLevel(sampleLinear, at / float2(lumaSize), 0);
        float2 uv = (flags & 32) != 0 ? at / float2(size) : clamp(at * 0.5, 0.5, visible * 0.5 - 0.5) / float2(size);
        samples.y = CB(planeCb.SampleLevel(sampleLinear, uv, 0));
        samples.z = CR(planeCb.SampleLevel(sampleLinear, uv, 0), planeCr.SampleLevel(sampleLinear, uv, 0));
    }
    if ((flags & 64) != 0) samples *= 65535.0 / 1023.0;
    HIGH_BITS
    float3 rgb = linearRgb(samples - 0.5);
    if (tonemap2 > 0) {
        float3 v = mul(bt709to2020, rgb);
        float vmax = max(v.r, max(v.g, v.b));
        if (vmax > 0) v *= (1 + tonemap1 * vmax) / (1 + tonemap2 * vmax);
        rgb = mul(bt2020to709, v);
    }
    rgb *= outputScale;
    if ((flags & 8192) != 0) return float4(gamma22FromScrgb(rgb, position) * fade, 1);
    if ((flags & 4096) != 0) return float4(pqFromScrgb(rgb * fade), 1);
    if ((flags & 1024) != 0) rgb = saturate(srgbFromLinear(rgb));
    return float4(rgb * fade, 1);
}
#endif
float4 planes(float4 position : SV_Position) : SV_Target {
#ifdef VULKAN
    if ((flags & 512) != 0) return planesDirect(position.xy);
#endif
    int3 p = int3(int2(position.xy), 0);
    float3 samples;
#ifdef VULKAN
    if ((flags & 2048) != 0) {
        samples = INTERLEAVED(planeCb.Load(p));
    } else
#endif
    {
        samples.x = planeY.Load(p);
        if ((flags & 32) != 0) {
            samples.y = CB(planeCb.Load(p));
            samples.z = CR(planeCb.Load(p), planeCr.Load(p));
        } else {
            // Chroma is half size: sample it where the CPU path does.
            uint2 size;
            planeCb.GetDimensions(size.x, size.y);
            float2 uv = clamp(position.xy * 0.5, 0.5, float2(width, height) * 0.5 - 0.5) / float2(size);
            samples.y = CB(planeCb.SampleLevel(sampleLinear, uv, 0));
            samples.z = CR(planeCb.SampleLevel(sampleLinear, uv, 0), planeCr.SampleLevel(sampleLinear, uv, 0));
        }
    }
    if ((flags & 64) != 0) samples *= 65535.0 / 1023.0;
    HIGH_BITS
    return float4(linearRgb(samples - 0.5), 1);
}

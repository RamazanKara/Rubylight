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
    float3 p = pow(saturate(rgb), 32.0 / 2523.0);
    rgb = pow(max(p - 3424.0 / 4096.0, 0) / max(2413.0 / 128.0 - 2392.0 / 128.0 * p, 0.000001), 16384.0 / 2610.0) * (125 / sdrWhite);
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
#ifdef VULKAN
Texture2D<float> planeY : register(t0, space2);
Texture2D<float> planeCb : register(t1, space2);
Texture2D<float> planeCr : register(t2, space2);
#else
Texture2D<float> planeY : register(t1);
Texture2D<float> planeCb : register(t2);
Texture2D<float> planeCr : register(t3);
#endif
float4 planes(float4 position : SV_Position) : SV_Target {
    int3 p = int3(int2(position.xy), 0);
    float3 samples;
    samples.x = planeY.Load(p);
    if ((flags & 32) != 0) {
        samples.y = planeCb.Load(p);
        samples.z = planeCr.Load(p);
    } else {
        // Chroma is half size: sample it where the CPU path does.
        uint2 size;
        planeCb.GetDimensions(size.x, size.y);
        float2 uv = clamp(position.xy * 0.5, 0.5, float2(width, height) * 0.5 - 0.5) / float2(size);
        samples.y = planeCb.SampleLevel(sampleLinear, uv, 0);
        samples.z = planeCr.SampleLevel(sampleLinear, uv, 0);
    }
    if ((flags & 64) != 0) samples *= 65535.0 / 1023.0;
    return float4(linearRgb(samples - 0.5), 1);
}

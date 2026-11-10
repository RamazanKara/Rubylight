// The present pass of a pass-through HDR10 swapchain that Nightjar describes itself
// (stream/wl_scanout.rs): pq_present.hlsl's pass with the encoding in constants the patched SDL
// pushes on each present (`SDL.renderer.gpu.present_encoding`, `_min_nits`, `_max_nits`,
// vendor/sdl3-src/patches/pq-present.patch). Mode 0 writes BT.2020 PQ as pq_present.hlsl does;
// mode 1 writes the compositor's gamma 2.2 blending space (BT.2020, minNits to the description's
// peak), which a compositor such as KWin 6.5 and later shows without converting each frame, so it
// can put the swapchain on a display plane. The vertex stage is pq_present.hlsl's.
// Vulkan only, compiled with glslang 16.3.0 (MSYS2's) as pyrowave.hlsl is:
//   glslangValidator -V -D -e fragment -S frag --auto-sampled-textures --target-env vulkan1.0 scanout_present.hlsl -o scanout_present_fragment.spv
// scanout_present_fragment.spv.sha256 records both hashes; a unit test compares them.
// SDL's fragment sampler slots are t0/s0 in space2, its fragment constants b0 in space3.

Texture2D<float4> frame : register(t0, space2);
SamplerState sampleLinear : register(s0, space2);
cbuffer Encoding : register(b0, space3) {
    float mode; float minNits; float invRange; float unused;
};

struct Varyings {
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
};

float3 pq(float3 nits) {
    const float m1 = 2610.0 / 16384.0;
    const float m2 = 2523.0 / 4096.0 * 128.0;
    const float c1 = 3424.0 / 4096.0;
    const float c2 = 2413.0 / 4096.0 * 32.0;
    const float c3 = 2392.0 / 4096.0 * 32.0;
    float3 y = pow(saturate(nits / 10000.0), m1);
    return pow((c1 + c2 * y) / (1.0 + c3 * y), m2);
}

// A 4x4 ordered dither in (0, 1), the same as pyrowave.hlsl's `orderedDither`.
static const float bayer[16] = { 0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5 };
float orderedDither(float2 position) {
    uint2 p = uint2(position) & 3;
    return (bayer[p.y * 4 + p.x] + 0.5) / 16.0;
}

// Gamma 2.2 over minNits..peak (invRange = 1 / (peak - minNits)), as color-management-v1 reads a
// parametric description's luminances, with half a 10-bit step of ordered dither: its coarse
// steps near black show as fine noise, not bands.
float3 gamma22(float3 nits, float2 position) {
    float3 encoded = pow(saturate((nits - minNits) * invRange), 1.0 / 2.2);
    return saturate(encoded + (orderedDither(position) - 0.5) / 1023.0);
}

float4 fragment(Varyings v) : SV_Target {
    float3 linear709 = frame.Sample(sampleLinear, v.uv).rgb;
    // BT.709 to BT.2020 primaries (ITU-R BT.2087).
    const float3x3 to2020 = float3x3(
        0.6274040, 0.3292820, 0.0433136,
        0.0690970, 0.9195400, 0.0113612,
        0.0163916, 0.0880132, 0.8955950);
    float3 nits = max(mul(to2020, linear709), 0.0) * 80.0;
    if (mode >= 0.5) return float4(gamma22(nits, v.position.xy), 1.0);
    return float4(pq(nits), 1.0);
}

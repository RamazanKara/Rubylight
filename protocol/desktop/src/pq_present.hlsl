// The PQ present pass of the patched SDL GPU renderer (vendor/sdl3-src/patches/pq-present.patch):
// SDL draws the frame in linear light (scRGB, 1.0 = 80 nits, BT.709 primaries) into its RGBA16F
// back buffer as for an scRGB swapchain, and this pass writes it to an HDR10 swapchain as BT.2020
// PQ (SMPTE ST 2084). Used where the swapchain offers HDR10 and not scRGB (sdl.rs, handheld.rs).
// Vulkan only, compiled with glslang (dxc is not used here):
//   glslangValidator -V -D -e vertex -S vert --target-env vulkan1.0 pq_present.hlsl -o pq_present_vertex.spv
//   glslangValidator -V -D -e fragment -S frag --auto-sampled-textures --target-env vulkan1.0 pq_present.hlsl -o pq_present_fragment.spv
// SDL's fragment sampler slots are t0/s0 in space2.

Texture2D<float4> frame : register(t0, space2);
SamplerState sampleLinear : register(s0, space2);

struct Varyings {
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
};

// One triangle covering the target; no vertex buffer. SDL's Vulkan viewport is flipped, so clip
// space has y up as in D3D: the texture's top row is y = 1.
Varyings vertex(uint id : SV_VertexID) {
    Varyings v;
    v.uv = float2((id << 1) & 2, id & 2);
    v.position = float4(v.uv.x * 2.0 - 1.0, 1.0 - v.uv.y * 2.0, 0.0, 1.0);
    return v;
}

float3 pq(float3 nits) {
    const float m1 = 2610.0 / 16384.0;
    const float m2 = 2523.0 / 4096.0 * 128.0;
    const float c1 = 3424.0 / 4096.0;
    const float c2 = 2413.0 / 4096.0 * 32.0;
    const float c3 = 2392.0 / 4096.0 * 32.0;
    float3 y = pow(saturate(nits / 10000.0), m1);
    return pow((c1 + c2 * y) / (1.0 + c3 * y), m2);
}

float4 fragment(Varyings v) : SV_Target {
    float3 linear709 = frame.Sample(sampleLinear, v.uv).rgb;
    // BT.709 to BT.2020 primaries (ITU-R BT.2087).
    const float3x3 to2020 = float3x3(
        0.6274040, 0.3292820, 0.0433136,
        0.0690970, 0.9195400, 0.0113612,
        0.0163916, 0.0880132, 0.8955950);
    float3 linear2020 = max(mul(to2020, linear709), 0.0);
    return float4(pq(linear2020 * 80.0), 1.0);
}

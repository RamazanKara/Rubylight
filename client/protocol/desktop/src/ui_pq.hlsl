// egui's meshes (the statistics overlay, a toast, the stream menu) on an HDR10 swapchain image
// taken in a video-only frame (vendor/sdl3-src/patches/window-target.patch, stream/ui_pq.rs).
// SDL's own draw of them writes linear light, which belongs in its RGBA16F back buffer and not in
// that image, so a frame with any of them went through the back buffer and its full-screen PQ
// pass (4K: two more full-screen passes over 16-bit float, about 2.5 ms of an Iris Plus 655's
// render engine a picture, rust/PERFORMANCE.md, November 12; reported from a Radeon 760M in
// nightjar issue 104). This fragment stage does what SDL's textured draw does
// (texture_rgba.frag.hlsl: the texel times the vertex colour, the colour scale on its RGB; scRGB,
// 1.0 = 80 nits, premultiplied alpha) and then encodes the colour as the present would have:
// mode 0 BT.2020 PQ (pq_present.hlsl, its matrix and curve exactly), mode 1 the compositor's
// gamma 2.2 blending space (scanout_present.hlsl, exactly). The alpha stays premultiplied, so
// the blend happens between encoded values, as a desktop blends sRGB, where the back buffer
// blended linear light: a translucent panel over a bright picture comes out a little darker.
// The vertex stage is SDL's own (tri_texture.vert.hlsl): colour, then texture coordinate.
// Vulkan only, compiled with glslang 16.3.0 (MSYS2's) as scanout_present.hlsl is:
//   glslangValidator -V -D -e fragment -S frag --auto-sampled-textures --target-env vulkan1.0 ui_pq.hlsl -o ui_pq_fragment.spv
// ui_pq_fragment.spv.sha256 records both hashes; a unit test compares them.
// SDL's fragment sampler slots are t0/s0 in space2, its fragment constants b0 in space3.

Texture2D<float4> mesh : register(t0, space2);
SamplerState meshSampler : register(s0, space2);
cbuffer Look : register(b0, space3) {
    // SDL's colour scale for the draw (the renderer's colour scale times the output's SDR white).
    float colorScale;
    float mode; float minNits; float invRange;
};

struct Fragment {
    float4 color : COLOR0;
    float2 uv : TEXCOORD0;
    float4 position : SV_Position;
};

static const float3x3 to2020 = float3x3(
    0.6274040, 0.3292820, 0.0433136,
    0.0690970, 0.9195400, 0.0113612,
    0.0163916, 0.0880132, 0.8955950);

float3 pq(float3 nits) {
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

float4 fragment(Fragment input) : SV_Target {
    float4 color = mesh.Sample(meshSampler, input.uv) * input.color;
    float alpha = saturate(color.a);
    // Nothing covered: nothing to encode (and no division by zero).
    if (alpha <= 0.0) return float4(0, 0, 0, 0);
    float3 nits = max(mul(to2020, color.rgb * colorScale / alpha), 0.0) * 80.0;
    float3 encoded;
    if (mode >= 0.5) {
        encoded = saturate(pow(saturate((nits - minNits) * invRange), 1.0 / 2.2) + (orderedDither(input.position.xy) - 0.5) / 1023.0);
    } else {
        encoded = pq(nits);
    }
    return float4(encoded * alpha, alpha);
}

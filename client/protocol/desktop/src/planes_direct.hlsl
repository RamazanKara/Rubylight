// Appended to pyrowave.hlsl for Direct3D 11 only (stream/d3d11.rs, `PlanesPass::draw_direct`), so
// the Vulkan module built from pyrowave.hlsl stays as it is. The entries here draw a picture's
// planes straight into the back buffer, converted and scaled in one pass: what the plane entries
// write into the RGBA64F video texture followed by SDL's own draw of that texture (stream/
// direct_planes.rs `Look`): bilinear scaling, its tone map when the texture's headroom is above
// the output's (TONEMAP_CHROME), its colour scale, and on an sRGB output its encoding and clamp.
// The Vulkan build does the same in `planesDirect` (planes_gpu.rs `direct_uniforms`).
cbuffer Direct : register(b1) {
    // The rectangle the picture fills (x, y, w, h in back buffer pixels) and the picture's
    // visible size in the planes (x, y).
    float4 destination; float4 picture;
    float outputScale; float tonemap1; float tonemap2;
    // Bit 0: the output is sRGB, not scRGB.
    uint directFlags;
};
// A D3D11VA 4:4:4 surface of the decoder, read in place (a slice of its texture array).
Texture2DArray<float4> surfaceArray : register(t5);
static const float3x3 directTo2020 = { 0.627404, 0.329283, 0.043313, 0.069097, 0.919541, 0.011362, 0.016391, 0.088013, 0.895595 };
static const float3x3 directTo709 = { 1.660496, -0.587656, -0.072840, -0.124547, 1.132895, -0.008348, -0.018154, -0.100597, 1.118751 };
// SDL's sRGBfromLinear, for an output that is not linear.
float3 directSrgb(float3 v) {
    return lerp(pow(abs(v), 1.0 / 2.4) * 1.055 - 0.055, v * 12.92, float3(v <= 0.0031308));
}
float4 directFinish(float3 rgb) {
    if (tonemap2 > 0) {
        float3 v = mul(directTo2020, rgb);
        float vmax = max(v.r, max(v.g, v.b));
        if (vmax > 0) v *= (1 + tonemap1 * vmax) / (1 + tonemap2 * vmax);
        rgb = mul(directTo709, v);
    }
    rgb *= outputScale;
    if ((directFlags & 1) != 0) rgb = saturate(directSrgb(rgb));
    return float4(rgb, 1);
}
// Where the target pixel is in the picture: its centre, kept inside the picture's texels.
float2 directAt(float2 position) {
    return clamp((position - destination.xy) * picture.xy / destination.zw, 0.5, picture.xy - 0.5);
}
float3 directSurfaceSamples(float4 texel) {
    float3 samples = (flags & 16) != 0 ? texel.grb : texel.bgr;
    if ((flags & 256) != 0) samples *= 65535.0 / 65472.0;
    return samples;
}
float4 planesDirect(float4 position : SV_Position) : SV_Target {
    float2 visible = picture.xy;
    float2 at = directAt(position.xy);
    uint2 size;
    uint2 lumaSize;
    planeCb.GetDimensions(size.x, size.y);
    planeY.GetDimensions(lumaSize.x, lumaSize.y);
    float3 samples;
    samples.x = planeY.SampleLevel(sampleLinear, at / float2(lumaSize), 0);
    float2 uv = (flags & 32) != 0 ? at / float2(size) : clamp(at * 0.5, 0.5, visible * 0.5 - 0.5) / float2(size);
    samples.y = planeCb.SampleLevel(sampleLinear, uv, 0);
    samples.z = planeCr.SampleLevel(sampleLinear, uv, 0);
    if ((flags & 64) != 0) samples *= 65535.0 / 1023.0;
    return directFinish(linearRgb(samples - 0.5));
}
// The copy of a 4:4:4 decoder picture (surface444.hlsl's `surface`, t4).
float4 surfaceDirect(float4 position : SV_Position) : SV_Target {
    uint2 size;
    surface.GetDimensions(size.x, size.y);
    float4 texel = surface.SampleLevel(sampleLinear, directAt(position.xy) / float2(size), 0);
    return directFinish(linearRgb(directSurfaceSamples(texel) - 0.5));
}
float4 surfaceArrayDirect(float4 position : SV_Position) : SV_Target {
    uint3 size;
    surfaceArray.GetDimensions(size.x, size.y, size.z);
    float4 texel = surfaceArray.SampleLevel(sampleLinear, float3(directAt(position.xy) / float2(size.xy), 0), 0);
    return directFinish(linearRgb(directSurfaceSamples(texel) - 0.5));
}
// The in-place surface converted into the RGBA64F texture, for what reads the texture.
float4 surfaceArray444(float4 position : SV_Position) : SV_Target {
    float4 texel = surfaceArray.Load(int4(int2(position.xy), 0, 0));
    return float4(linearRgb(directSurfaceSamples(texel) - 0.5), 1);
}

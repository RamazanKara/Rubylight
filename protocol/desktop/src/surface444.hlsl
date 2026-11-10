// Appended to pyrowave.hlsl for Direct3D 11 only (stream/d3d11.rs, `PlanesPass::draw_surface`), so
// the Vulkan module built from pyrowave.hlsl stays as it is. A D3D11VA 4:4:4 picture's copy is
// read through one view and converted by pyrowave.hlsl's linearRgb, with the range, matrix and
// transfer flags of the planes entry. AYUV is read as R8G8B8A8 (Cr, Cb, Y, A). With flag 16, Y410
// as R10G10B10A2 or Y416 as R16G16B16A16 (Cb, Y, Cr, A), and with flag 256 Y416's 10-bit samples
// in the high bits of each word (code * 64 / 65535), which P010's planes have too.
Texture2D<float4> surface : register(t4);
float4 surface444(float4 position : SV_Position) : SV_Target {
    float4 texel = surface.Load(int3(int2(position.xy), 0));
    float3 samples = (flags & 16) != 0 ? texel.grb : texel.bgr;
    if ((flags & 256) != 0) samples *= 65535.0 / 65472.0;
    return float4(linearRgb(samples - 0.5), 1);
}

// Draws a decoded NV12 or P010 picture straight into SDL's back buffer (stream/direct.rs), from a
// slice of the decoder's texture array or from the SDL video texture, with the color conversion
// and tone mapping of SDL's D3D11 renderer (D3D11_PixelShader_Common.hlsli). The tone map's
// headroom comes per frame from Dolby Vision, HDR10+ or the measured peak (`peak`), which SDL
// cannot take, and HLG pictures (which SDL cannot draw) are rendered for the display's peak.

Texture2DArray<float> luma : register(t0);
Texture2DArray<float2> chroma : register(t1);
SamplerState linearClamp : register(s0);
RWByteAddressBuffer histogram : register(u0);

cbuffer Draw : register(b0) {
    float2 picture; float2 textureSize;
    float slice; uint flags; float sdrWhite; float colorScale;
    float tonemap1; float tonemap2; float hlgPeak; float hlgGamma;
};

// flags: bit 0 10-bit, bit 1 full range, bits 2-3 matrix (0 BT.601, 1 BT.709, 2 BT.2020),
// bit 4 PQ, bit 5 linear (scRGB) output, bit 6 tone map, bit 7 HLG.
bool flag(uint bit) { return (flags >> bit & 1) != 0; }

float4 vertex(uint id : SV_VertexID, out float2 uv : TEXCOORD0) : SV_Position {
    float2 p = float2((id << 1) & 2, id & 2);
    uv = p * picture / textureSize;
    return float4(p * float2(2, -2) + float2(-1, 1), 0, 1);
}

// Nonlinear R'G'B' from samples of the planes, as the codes say (packed444.hlsl's linearRgb).
float3 rgbOf(float y, float2 c) {
    bool ten = flag(0);
    bool full = flag(1);
    uint matrixKind = flags >> 2 & 3;
    float scale = ten ? 65535.0 / 64 : 255;
    float factor = ten ? 4 : 1;
    float maximum = ten ? 1023 : 255;
    float luma = (y * scale - (full ? 0 : 16 * factor)) / (full ? maximum : 219 * factor);
    float2 uv = (c * scale - 128 * factor) / (full ? maximum : 224 * factor);
    float3 k = matrixKind == 2 ? float3(0.2627, 0.6780, 0.0593) :
               matrixKind == 0 ? float3(0.299, 0.587, 0.114) : float3(0.2126, 0.7152, 0.0722);
    float r = luma + 2 * (1 - k.r) * uv.y;
    float b = luma + 2 * (1 - k.b) * uv.x;
    return float3(r, (luma - k.r * r - k.b * b) / k.g, b);
}

float3 pqToNits(float3 v) {
    float3 p = pow(abs(v), 1.0 / 78.84375);
    return 10000.0 * pow(max(p - 0.8359375, 0) / (18.8515625 - 18.6875 * p), 1.0 / 0.1593017578125);
}

// BT.2100 HLG: the inverse OETF to scene light, then the OOTF for a display of hlgPeak nits
// (direct.rs `hlg_nits`).
float3 hlgToNits(float3 v) {
    float3 scene = v <= 0.5 ? v * v / 3 : (exp((v - 0.55991073) / 0.17883277) + 0.28466892) / 12;
    float luma = dot(scene, float3(0.2627, 0.6780, 0.0593));
    return luma > 0 ? hlgPeak * pow(luma, hlgGamma - 1) * scene : 0;
}

float srgbToLinear(float v) { return v <= 0.04045 ? v / 12.92 : pow(abs(v + 0.055) / 1.055, 2.4); }
float srgbFromLinear(float v) { return v <= 0.0031308 ? v * 12.92 : pow(abs(v), 1.0 / 2.4) * 1.055 - 0.055; }

static const float3x3 bt2020To709 = {
    { 1.660496, -0.587656, -0.072840 },
    { -0.124547, 1.132895, -0.008348 },
    { -0.018154, -0.100597, 1.118751 }
};

float4 draw(float4 position : SV_Position, float2 uv : TEXCOORD0) : SV_Target {
    // Keep the bilinear taps inside the picture: the decoder's surfaces are taller than it.
    float2 lumaAt = clamp(uv, 0.5 / textureSize, (picture - 0.5) / textureSize);
    float2 chromaAt = clamp(uv, 1.0 / textureSize, (picture - 1.0) / textureSize);
    float3 rgb = rgbOf(luma.Sample(linearClamp, float3(lumaAt, slice)),
                       chroma.Sample(linearClamp, float3(chromaAt, slice)));
    if (flag(4) || flag(7)) {
        rgb = (flag(7) ? hlgToNits(saturate(rgb)) : pqToNits(saturate(rgb))) / sdrWhite;
        if (flag(6)) {
            float peak = max(rgb.r, max(rgb.g, rgb.b));
            if (peak > 0) rgb *= (1 + tonemap1 * peak) / (1 + tonemap2 * peak);
        }
        rgb = mul(bt2020To709, rgb) * colorScale;
        if (!flag(5)) {
            rgb = saturate(float3(srgbFromLinear(rgb.r), srgbFromLinear(rgb.g), srgbFromLinear(rgb.b)));
        }
    } else {
        if (flag(5)) rgb = float3(srgbToLinear(rgb.r), srgbToLinear(rgb.g), srgbToLinear(rgb.b));
        rgb *= colorScale;
    }
    return float4(rgb, 1);
}

// A histogram of the PQ-coded maxRGB of every 4th pixel each way, 256 bins, for the peak the
// tone map follows when the stream carries no HDR10+.
[numthreads(8, 8, 1)]
void peak(uint3 id : SV_DispatchThreadID) {
    uint2 p = id.xy * 4 + 1;
    if (p.x >= (uint)picture.x || p.y >= (uint)picture.y) return;
    float3 rgb = saturate(rgbOf(luma.Load(int4(p, slice, 0)), chroma.Load(int4(p / 2, slice, 0))));
    uint bin = min(uint(max(rgb.r, max(rgb.g, rgb.b)) * 256), 255);
    histogram.InterlockedAdd(bin * 4, 1);
}

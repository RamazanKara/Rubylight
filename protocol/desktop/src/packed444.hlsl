#ifdef VULKAN
StructuredBuffer<uint> samples : register(t0, space2);
cbuffer Picture : register(b0, space3) {
#else
Texture2D<float> planeY : register(t0);
Texture2D<float2> planeUV : register(t1);
cbuffer Picture : register(b0) {
#endif
    uint width; uint height; uint paddedWidth; uint paddedHeight;
    uint tenBit; uint fullRange; uint colorMatrix; uint hdr;
    float sdrWhite; float padding0; float padding1; float padding2;
};

#ifdef VULKAN
float sampleCode(uint at) {
    uint word = samples[tenBit != 0 ? at / 2 : at / 4];
    return tenBit != 0 ? float((word >> ((at % 2) * 16 + 6)) & 1023)
                       : float((word >> ((at % 4) * 8)) & 255);
}
float readY(uint2 p) { return sampleCode(p.y * paddedWidth + p.x); }
float2 readUV(uint2 p) {
    uint at = 2 * paddedWidth * paddedHeight + p.y * paddedWidth + 2 * p.x;
    return float2(sampleCode(at), sampleCode(at + 1));
}
#else
float readY(uint2 p) { return planeY.Load(int3(p, 0)) * (tenBit != 0 ? 65535.0 / 64 : 255); }
float2 readUV(uint2 p) { return planeUV.Load(int3(p, 0)) * (tenBit != 0 ? 65535.0 / 64 : 255); }
float4 vertex(uint id : SV_VertexID) : SV_Position {
    float2 p = float2((id << 1) & 2, id & 2);
    return float4(p * float2(2, -2) + float2(-1, 1), 0, 1);
}
#endif

float3 linearRgb(float3 yuv) {
    float maximum = tenBit != 0 ? 1023 : 255;
    float factor = tenBit != 0 ? 4 : 1;
    float y = (yuv.x - (fullRange != 0 ? 0 : 16 * factor)) / (fullRange != 0 ? maximum : 219 * factor);
    float2 uv = (yuv.yz - 128 * factor) / (fullRange != 0 ? maximum : 224 * factor);
    float3 k = hdr != 0 || colorMatrix == 2 ? float3(0.2627, 0.6780, 0.0593) :
               colorMatrix == 0 ? float3(0.299, 0.587, 0.114) : float3(0.2126, 0.7152, 0.0722);
    float r = y + 2 * (1 - k.r) * uv.y;
    float b = y + 2 * (1 - k.b) * uv.x;
    float3 rgb = float3(r, (y - k.r * r - k.b * b) / k.g, b);
    if (hdr == 0) {
        rgb = saturate(rgb);
        return lerp(pow((rgb + 0.055) / 1.055, 2.4), rgb / 12.92, step(rgb, 0.04045));
    }
    float3 p = pow(saturate(rgb), 32.0 / 2523.0);
    rgb = pow(max(p - 3424.0 / 4096.0, 0) / max(2413.0 / 128.0 - 2392.0 / 128.0 * p, 0.000001), 16384.0 / 2610.0) * (125 / sdrWhite);
    return float3(dot(rgb, float3(1.660491, -0.587641, -0.072850)),
                  dot(rgb, float3(-0.124550, 1.132900, -0.008349)),
                  dot(rgb, float3(-0.018151, -0.100579, 1.118730)));
}

float4 fragment(float4 position : SV_Position) : SV_Target {
    uint2 p = min(uint2(position.xy), uint2(width - 1, height - 1));
    float2 uv;
    if ((p.x & 1) == 0) uv = readUV(uint2(p.x / 2, p.y));
    else uv = float2(readY(uint2(p.x / 2, paddedHeight + p.y)),
                     readY(uint2(paddedWidth / 2 + p.x / 2, paddedHeight + p.y)));
    return float4(linearRgb(float3(readY(p), uv)), 1);
}

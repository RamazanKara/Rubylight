// RTX Video enhancement of an HDR (FP16 scRGB) picture through NVIDIA's SDR
// model, at the capture's own size. Ported in part from Foundation Sunshine's
// DLSS NR HDR filter (AlkaidLab/foundation-sunshine, GPL-3.0,
// src/platform/windows/image_enhancement/dlss_nr/hdr_filter.cpp): the model
// sees an SDR proxy normalized around 203-nit white with highlights
// compressed, and the resolve pass adds back only the bounded change the
// model made, decoded from the exact quantized proxy, so an unchanged proxy
// leaves the original pixel exactly as it was.
Texture2D<float4> original : register(t0);
Texture2D<float4> proxy : register(t1);
Texture2D<float4> enhanced : register(t2);

float3 decode(float3 c) {
  return lerp(c / 12.92, pow(max((c + 0.055) / 1.055, 0), 2.4), step(0.04045, c));
}
float3 encode(float3 c) {
  return lerp(c * 12.92, 1.055 * pow(max(c, 0), 1.0 / 2.4) - 0.055, step(0.0031308, c));
}
// scRGB 1.0 is 80 nits: 2.5375 is 203 nits.
float proxy_scale(float3 c) { return 2.5375 + max(0, max(c.r, max(c.g, c.b))); }

float4 vertex(uint id : SV_VertexID) : SV_Position {
  return float4(id == 2 ? 3 : -1, id == 1 ? 3 : -1, 0, 1);
}
// The SDR proxy the model enhances (BGRA8 target).
float4 prepare(float4 position : SV_Position) : SV_Target {
  float3 c = original.Load(int3(uint2(position.xy), 0)).rgb;
  return float4(saturate(encode(max(c, 0) / proxy_scale(c))), 1);
}
// The original FP16 pixel with the model's bounded change added back.
float4 resolve(float4 position : SV_Position) : SV_Target {
  int3 at = int3(uint2(position.xy), 0);
  float4 source = original.Load(at);
  float3 delta = decode(enhanced.Load(at).rgb) - decode(proxy.Load(at).rgb);
  float3 result = clamp(source.rgb + clamp(delta, -0.25, 0.25) * proxy_scale(source.rgb), -65504, 65504);
  return float4(all(isfinite(result)) ? result : source.rgb, source.a);
}

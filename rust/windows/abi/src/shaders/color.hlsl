
Texture2D<float4> source : register(t0);
Texture2D<float4> pointer : register(t1);
cbuffer Config : register(b0) {
    uint2 sourceSize; uint2 targetSize;
    uint pixel; uint hdr; uint colorMatrix; uint fullRange;
    uint tenBit; float sdrWhiteScale; float hdrScale; uint padding;
    int2 pointerPosition; uint2 pointerSize;
    uint pointerMode;
    // SDR streams of FP16 and PQ captures: `toneMap` is `hdr_sdr_tone_map`,
    // and `sdrDesktop` marks an FP16 capture whose display is not in HDR.
    // `toneMapPeak` is log2 of the source's peak in nits (`method_b`).
    uint toneMap; uint sdrDesktop; float toneMapPeak;
};
float4 vertex(uint id : SV_VertexID) : SV_Position {
    float2 p = float2((id << 1) & 2, id & 2);
    return float4(p * float2(2, -2) + float2(-1, 1), 0, 1);
}
float3 srgb_linear(float3 rgb) { return lerp(pow((rgb + 0.055) / 1.055, 2.4), rgb / 12.92, step(rgb, 0.04045)); }
float3 gamma(float3 rgb) { return lerp(1.055 * pow(max(rgb, 0), 1.0 / 2.4) - 0.055, rgb * 12.92, step(rgb, 0.0031308)); }
// BT.2020's 10-bit SDR OETF (transfer_characteristics = 14), not sRGB or PQ.
float3 bt2020_sdr(float3 light) {
    light = saturate(light);
    return lerp(1.099 * pow(light, 0.45) - 0.099, 4.5 * light, step(light, 0.018));
}
float3 gamut2020(float3 rgb) {
    return float3(dot(rgb, float3(0.627404, 0.329283, 0.043313)),
                  dot(rgb, float3(0.069097, 0.919540, 0.011362)),
                  dot(rgb, float3(0.016391, 0.088013, 0.895595)));
}
// BT.2020 to BT.709 primaries, the inverse of `gamut2020`.
float3 gamut709(float3 rgb) {
    return float3(dot(rgb, float3(1.660491, -0.587641, -0.072850)),
                  dot(rgb, float3(-0.124550, 1.132900, -0.008349)),
                  dot(rgb, float3(-0.018151, -0.100579, 1.118730)));
}
float3 pq(float3 luminance) {
    float3 power = pow(clamp(luminance, 0, 1), 2610.0 / 16384.0);
    return pow((3424.0 / 4096.0 + 2413.0 / 128.0 * power) /
               (1.0 + 2392.0 / 128.0 * power), 2523.0 / 32.0);
}
float pq1(float luminance) {
    float power = pow(clamp(luminance, 0, 1), 2610.0 / 16384.0);
    return pow((3424.0 / 4096.0 + 2413.0 / 128.0 * power) /
               (1.0 + 2392.0 / 128.0 * power), 2523.0 / 32.0);
}
float3 depq(float3 rgb) {
    float3 p = pow(clamp(rgb, 0, 1), 32.0 / 2523.0);
    return pow(max(p - 3424.0 / 4096.0, 0) / max(2413.0 / 128.0 - 2392.0 / 128.0 * p, 0.000001), 16384.0 / 2610.0);
}
// BT.2100 HLG (ARIB STD-B67, `hdr == 2`) from BT.2020 display light in scRGB
// units (1.0 = 80 nits), graded for a fixed 1000-nit display: the inverse
// reference OOTF at system gamma 1.2, then the OETF. Light above 1000 nits
// keeps its headroom until the codes clamp. After Foundation Sunshine's
// scRGBTo2100HLG, DisplayLinearToHLGScene and LinearToHLG
// (src_assets/windows/assets/shaders/directx/include/common.hlsl,
// AlkaidLab/foundation-sunshine, commit 880f1ef). Copyright (c) Foundation
// Sunshine authors, GPL-3.0. `color::hlg` is the CPU reference.
float3 hlg(float3 light) {
    float3 display = max(light, 0) * (80.0 / 1000.0);
    float y = dot(display, float3(0.2627, 0.6780, 0.0593));
    float3 scene = y > 0 ? display * pow(max(y, 1e-6), (1.0 - 1.2) / 1.2) : 0;
    return lerp(0.17883277 * log(max(12.0 * scene - 0.28466892, 1e-6)) + 0.55991073,
                sqrt(3.0 * scene), step(scene, 1.0 / 12.0));
}
// An HDR desktop to an SDR stream: ITU-R BT.2446-1 Method B (section 5.2),
// ported from Foundation Sunshine's MethodBScRgbToSdr and LimitSdrGamut
// (src_assets/windows/assets/shaders/directx/include/convert_linear_base.hlsl,
// AlkaidLab/foundation-sunshine, commit 9d761ba, PR #1387). Copyright (c)
// Foundation Sunshine authors, GPL-3.0. `color::method_b` is the CPU reference.
// A PQ capture enters as scRGB too (`load`).
bool tone_mapped() { return hdr == 0 && pixel != 0 && toneMap != 0 && sdrDesktop == 0; }
// BT.2100 LMS matrix composed with the linear primary conversions.
static const float3x3 rec709ToLms = {
    0.295809355, 0.623101305, 0.081089341,
    0.156248976, 0.727325725, 0.116425299,
    0.035135663, 0.156574727, 0.808289610
};
static const float3x3 lmsToRec709 = {
     6.173362742, -5.320380568,  0.147017826,
    -1.324311500,  2.560226421, -0.235914921,
    -0.011816903, -0.264672035,  1.276488938
};
static const float3x3 lmsToSmpteC = {
     6.650457157, -5.807419747,  0.156962590,
    -1.493468713,  2.762204921, -0.268736208,
    -0.007513590, -0.260458703,  1.267972293
};
// Section 5.2.4: reduce chroma at constant ICtCp intensity and hue until the
// colour intersects the target RGB cube. ICtCp is linear in PQ-encoded LMS:
// scaling Ct and Cp together is interpolating from (I,I,I). Five bisection
// steps, then the cube's face along the chord between the last colours inside
// and outside, instead of Sunshine's twelve steps: as close to the converged
// intersection for under half the cost (rust/PERFORMANCE.md).
float3 limit_sdr_gamut(float3 rgb709, float3 target, float3x3 lmsToTarget) {
    [branch] if (all(target >= 0) && all(target <= 1)) return target;
    if (all(target >= 1)) return float3(1, 1, 1);
    float3 lmsPq = pq(mul(rec709ToLms, rgb709) / 100.0);
    float intensity = (lmsPq.x + lmsPq.y) * 0.5;
    if (intensity >= 0.50807842) return float3(1, 1, 1); // PQ at 100 nits
    float lo = 0, hi = 1;
    // The grey of this intensity, and the colour at full chroma.
    float3 inner = depq(float3(intensity, intensity, intensity)) * 100.0;
    float3 outer = target;
    [loop] for (int i = 0; i < 5; ++i) {
        float mid = (lo + hi) * 0.5;
        float3 candidate = mul(lmsToTarget, depq(lerp(intensity, lmsPq, mid)) * 100.0);
        if (all(candidate >= 0) && all(candidate <= 1)) {
            lo = mid;
            inner = candidate;
        } else {
            hi = mid;
            outer = candidate;
        }
    }
    // PQ clips negative LMS, so full chroma can already be inside the cube;
    // then there is no face to meet.
    if (hi == 1 && any(mul(rec709ToLms, rgb709) < 0)) return saturate(target);
    float3 chord = outer - inner;
    float3 reach = outer > 1 ? (1 - inner) / chord : (outer < 0 ? -inner / chord : 1);
    return saturate(inner + chord * saturate(min(reach.x, min(reach.y, reach.z))));
}
// scRGB to the stream's SDR signal in the primaries of `colorMatrix`.
float3 method_b(float3 rgb) {
    // Changing only Y at fixed u'v' scales all linear components equally,
    // negative ones included.
    float y = dot(rgb, float3(0.212639, 0.715169, 0.072192));
    if (y <= 0) return float3(0, 0, 0);
    // Invert the HLG reference OOTF of a display of the source's peak L_W,
    // at BT.2100's system gamma 1.2 + 0.42 log10(L_W / 1000), then render
    // the same scene light at 291 nits with gamma 1.0 (section 5.2.1, which
    // fixes L_W at 1000 nits). With `toneMapPeak` = log2(L_W) that is
    // 291 (80 y / L_W)^(1 / gamma), the peak mapping to SDR white.
    float systemGamma = 1.2 + 0.1264326 * (toneMapPeak - 9.9657843);
    float bridge = 291.0 * exp2((log2(y * 80.0) - toneMapPeak) / systemGamma);
    // 78 % BT.1886 signal is 55.0842 nits. The log shoulder's width joins it
    // with unit slope and maps 291 to nominal SDR white.
    const float knee = 55.0842;
    const float width = 16.44899;
    float nits = bridge <= knee ? bridge : knee + width * log(1.0 + (bridge - knee) / width);
    float3 mapped = rgb * (nits / 100.0 / y);
    float3 sdr = limit_sdr_gamut(mapped, mapped, lmsToRec709);
    if (colorMatrix == 0) { // SMPTE 170M, D65
        const float3x3 toSmpteC = {
             1.065379, -0.055401, -0.009978,
            -0.019633,  1.036363, -0.016730,
             0.001632,  0.004412,  0.993956
        };
        sdr = limit_sdr_gamut(sdr, mul(toSmpteC, sdr), lmsToSmpteC);
    } else if (colorMatrix == 2) {
        sdr = gamut2020(sdr);
    }
    // Inverse BT.1886, black 0, white 100 nits.
    return pow(max(sdr, 0), 1.0 / 2.4);
}
float3 pointer_srgb(float3 rgb) {
    if (pixel == 1) return saturate(gamma(rgb / sdrWhiteScale));
    if (pixel == 2) {
        float3 r = depq(rgb) * 125.0 / sdrWhiteScale;
        return saturate(gamma(float3(dot(r, float3(1.660491,-0.587641,-0.072850)),
                                     dot(r, float3(-0.124550,1.132900,-0.008349)),
                                     dot(r, float3(-0.018151,-0.100579,1.118730)))));
    }
    return rgb;
}
float3 pointer_source(float3 rgb) {
    if (pixel == 1) return srgb_linear(rgb) * sdrWhiteScale;
    if (pixel == 2) return pq(gamut2020(srgb_linear(rgb)) * sdrWhiteScale / 125.0);
    return rgb;
}
float3 load(int2 p) {
    p = clamp(p, int2(0, 0), int2(sourceSize) - 1);
    float3 rgb = source.Load(int3(p, 0)).rgb;
    int2 at = p - pointerPosition;
    if (pointerMode != 0 && all(at >= 0) && all(at < int2(pointerSize))) {
        float4 c = pointer.Load(int3(at, 0));
        if (pointerMode == 2 && c.a > 0.25 && c.a < 0.75) {
            uint3 bits = uint3(round(saturate(pointer_srgb(rgb)) * 255)) ^ uint3(round(c.rgb * 255));
            rgb = pointer_source(float3(bits) / 255);
        } else if (pixel == 2) {
            rgb = pq(lerp(depq(rgb), gamut2020(srgb_linear(c.rgb)) * sdrWhiteScale / 125.0, c.a));
        } else {
            rgb = lerp(rgb, pointer_source(c.rgb), c.a);
        }
    }
    if (hdr != 0 && pixel == 0) {
        rgb = srgb_linear(rgb);
        rgb *= sdrWhiteScale;
    }
    if (hdr != 0 && pixel == 1) rgb *= hdrScale;
    // A tone-mapped PQ capture becomes scRGB here, so resizing averages
    // light and Method B sees what an FP16 capture of that desktop gives.
    if (pixel == 2 && tone_mapped()) rgb = gamut709(depq(rgb) * 125.0);
    // Normalize scRGB to SDR white. A BT.2020 stream keeps linear, extended
    // Rec.709 values until gamut mapping; clipping to sRGB here would lose
    // wide-gamut colors that are still inside BT.2020. A tone-mapped stream
    // keeps scRGB here and converts after resizing (`sample_source`).
    if (hdr == 0 && pixel == 1 && !tone_mapped()) {
        rgb /= sdrWhiteScale;
        if (colorMatrix != 2) rgb = gamma(saturate(rgb));
    }
    return rgb;
}
// The source keeps its aspect ratio, centred in the stream with black bars
// (a 32:9 desktop streamed to a 16:10 tablet). Sizes within a pixel of the
// stream's shape still fill it, so rounding never adds a one-pixel line.
bool content(float2 target, out float2 p, out float2 footprint) {
    float2 ratio = float2(targetSize) / float2(sourceSize);
    float scale = min(ratio.x, ratio.y);
    float2 size = float2(sourceSize) * scale;
    if (all(abs(size - float2(targetSize)) < 1)) {
        p = (target + 0.5) / ratio - 0.5;
        footprint = 1 / ratio;
        return true;
    }
    float2 at = target + 0.5 - (float2(targetSize) - size) * 0.5;
    p = at / scale - 0.5;
    footprint = 1 / scale;
    return all(at >= 0) && all(at < size);
}
float3 bilinear(float2 p) {
    int2 at = int2(floor(p));
    float2 f = frac(p);
    return lerp(lerp(load(at), load(at + int2(1, 0)), f.x),
                lerp(load(at + int2(0, 1)), load(at + int2(1, 1)), f.x), f.y);
}
// Integrate source-pixel coverage when reducing an image. A fixed bilinear
// footprint skips thin features at fractional and >2x reductions, so they
// shimmer as the source moves. Native size and enlargement keep their old
// paths. An aligned 2x reduction already has the exact four-sample average.
float3 resize(float2 p, float2 footprint) {
    float2 left = p + 0.5 - footprint * 0.5;
    if (any(footprint < 1) || all(footprint <= 1) || any(footprint > 8) ||
        (all(footprint == 2) && all(frac(left) == 0))) return bilinear(p);
    float2 right = left + footprint;
    int2 first = int2(floor(left));
    int2 count = int2(ceil(right)) - first;
    float3 rgb = 0;
    // At most nine source pixels intersect a footprint of up to eight.
    // Keep a hard bound even for extreme stream/source size ratios; those
    // retain the previous bilinear path instead of an unbounded shader loop.
    [loop] for (int y = 0; y < 9; ++y) {
        if (y >= count.y) break;
        float wy = max(0, min(right.y, first.y + y + 1) - max(left.y, first.y + y));
        [loop] for (int x = 0; x < 9; ++x) {
            if (x >= count.x) break;
            float wx = max(0, min(right.x, first.x + x + 1) - max(left.x, first.x + x));
            rgb += load(first + int2(x, y)) * (wx * wy);
        }
    }
    return rgb / (footprint.x * footprint.y);
}
float3 sample_source(float2 target) {
    float3 rgb;
    if (all(sourceSize == targetSize)) {
        rgb = load(int2(target));
    } else if (all(sourceSize == targetSize * 2)) {
        // A uniform shape test avoids per-pixel coverage/control-flow work on
        // the common 4K -> 1080p path. Its four samples are already exact.
        rgb = bilinear(target * 2 + 0.5);
    } else if (all(sourceSize == targetSize * 3)) {
        // The other common integer reduction (4K -> 720p) covers nine whole
        // source pixels. Fixed unweighted loops keep the row-order sum without
        // duplicating the cursor/HDR load logic into every sample in bytecode.
        int2 at = int2(target) * 3;
        rgb = 0;
        [loop] for (int y = 0; y < 3; ++y) {
            [loop] for (int x = 0; x < 3; ++x) {
                rgb += load(at + int2(x, y));
            }
        }
        rgb /= 9;
    } else {
        float2 p;
        float2 footprint;
        if (!content(target, p, footprint)) return float3(0, 0, 0);
        rgb = resize(p, footprint);
    }
    [branch] if (tone_mapped()) rgb = method_b(rgb);
    return rgb;
}
// The SDR signal of `sample_source`'s colour; a tone-mapped one already is.
float3 sdr_signal(float3 rgb) {
    if (colorMatrix != 2 || tone_mapped()) return rgb;
    return bt2020_sdr(gamut2020(pixel == 1 ? rgb : srgb_linear(rgb)));
}
float3 nonlinear(float2 target) {
    float3 rgb = sample_source(target);
    if (hdr == 0) {
        // SDR desktop RGB uses sRGB transfer and Rec.709 primaries. Applying
        // only the BT.2020 YUV coefficients would relabel those primaries.
        return sdr_signal(rgb);
    }
    if (hdr == 2) return hlg(pixel == 2 ? depq(rgb) * 125.0 : gamut2020(rgb));
    if (pixel == 2) return rgb;
    return pq(gamut2020(rgb) / 125.0);
}
// The couch menu's panel (docs/couch-menu-design.md 6.2 and 6.3): opaque
// RGBA8 sRGB at 1x, read as UNORM, magnified by `menuScale` over the
// `menuSize` target pixels from `menuOrigin`. Its own cbuffer (b2, beside
// `Focus` at b1), so `Config` and the closed entries' bytecode stay as they
// were; the order is `Overlay::constants`.
cbuffer Menu : register(b2) {
    uint2 menuOrigin; uint2 menuSize;
    uint2 menuOriginBlocks; uint2 menuBlocks;
    uint menuScale; uint3 menuPadding;
};
Texture2D<float4> menuImage : register(t2);
// A panel pixel in the space `nonlinear` returns; HDR puts panel white at the
// SDR white level, as the cursor does. Clamping into the panel keeps the
// chroma filter's left neighbour on its border.
float3 menu_rgb(float2 target) {
    int2 at = clamp((int2(target) - int2(menuOrigin)) / int(menuScale), 0, int2(menuSize / menuScale) - 1);
    float3 c = menuImage.Load(int3(at, 0)).rgb;
    if (hdr == 2) return hlg(gamut2020(srgb_linear(c)) * sdrWhiteScale);
    if (hdr != 0) return pq(gamut2020(srgb_linear(c)) * sdrWhiteScale / 125.0);
    return colorMatrix == 2 ? bt2020_sdr(gamut2020(srgb_linear(c))) : c;
}
// A pixel's colour: the capture's, or the panel's in a menu pass. Every body
// passes a literal `menu`, which fxc folds, so the closed entries keep their
// code and the menu entries share the writers' layout and codes.
float3 rgb_at(float2 target, bool menu) {
    if (menu) return menu_rgb(target);
    return nonlinear(target);
}
// A menu pixel shader leaves every sample not taken from the panel, `at`, to
// the closed pass that drew the plane before it.
void menu_clip(uint2 at) {
    if (any(at < menuOrigin) || any(at >= menuOrigin + menuSize)) discard;
}
float3 weights() { return colorMatrix == 2 ? float3(0.2627, 0.6780, 0.0593) : colorMatrix == 0 ? float3(0.299, 0.587, 0.114) : float3(0.2126, 0.7152, 0.0722); }
// The luma code of a pixel, and the chroma code of filtered nonlinear RGB.
// Shared by the graphics and compute conversions, so both write the same codes.
float lumaCode(float3 rgb) {
    float y = dot(rgb, weights());
    return tenBit != 0 ? floor(clamp((fullRange != 0 ? 1023 * y : 64 + 876 * y), 0, 1023) + 0.5) * 64 / 65535.0
                       : floor(clamp((fullRange != 0 ? 255 * y : 16 + 219 * y), 0, 255) + 0.5) / 255.0;
}
float2 chromaCode(float3 rgb) {
    float3 k = weights();
    float y = dot(rgb, k);
    float2 uv = float2((rgb.b - y) / (2 * (1 - k.b)), (rgb.r - y) / (2 * (1 - k.r)));
    return tenBit != 0 ? floor(clamp(512 + (fullRange != 0 ? 1023 : 896) * uv, 0, 1023) + 0.5) * 64 / 65535.0
                       : floor(clamp(128 + (fullRange != 0 ? 255 : 224) * uv, 0, 255) + 0.5) / 255.0;
}
// Standard-codec 4:2:0 is horizontally co-sited with the left luma sample
// and vertically centred. A symmetric [1,2,1] x [1,1] filter preserves that
// position while suppressing aliasing. Clamp in target coordinates so the
// left edge also works with resizing and letterboxing.
// HDR streams filter light, not codes: PQ and HLG are so steep that averaging
// the codes of ClearType's coloured glyph fringes gave small text a magenta
// cast that SDR's gentler curve hides (Rubylight #130). Linear captures stay
// in BT.2020 light through the filter (`chroma_sample_at`) and encode once
// (`chromaCodes`), avoiding a transfer-and-inverse round-trip on every tap;
// SDR keeps filtering its codes.
// Branches, not `?:`, which would evaluate every curve for every stream.
float3 chroma_sample_at(float2 target, bool menu) {
    if (menu) {
        int2 at = clamp((int2(target) - int2(menuOrigin)) / int(menuScale), 0, int2(menuSize / menuScale) - 1);
        float3 c = menuImage.Load(int3(at, 0)).rgb;
        [branch] if (hdr == 0) return colorMatrix == 2 ? bt2020_sdr(gamut2020(srgb_linear(c))) : c;
        float3 light = gamut2020(srgb_linear(c)) * sdrWhiteScale;
        [branch] if (hdr == 2) return max(light, 0);
        return saturate(light / 125.0);
    }
    float3 rgb = sample_source(target);
    [branch] if (hdr == 0) {
        return sdr_signal(rgb);
    }
    [branch] if (hdr == 2) return pixel == 2 ? depq(rgb) * 125.0 : max(gamut2020(rgb), 0);
    [branch] if (pixel == 2) return depq(rgb);
    return saturate(gamut2020(rgb) / 125.0);
}
// Compute both the nonlinear code (returned) and the chroma filter's
// working-space value (`light`), evaluating `sample_source` once per pixel
// without a `pq -> depq` or `hlg -> hlgLight` round-trip.
float3 pixel_and_light(float2 target, bool menu, out float3 light) {
    if (menu) {
        int2 at = clamp((int2(target) - int2(menuOrigin)) / int(menuScale), 0, int2(menuSize / menuScale) - 1);
        float3 c = menuImage.Load(int3(at, 0)).rgb;
        [branch] if (hdr == 0) {
            light = colorMatrix == 2 ? bt2020_sdr(gamut2020(srgb_linear(c))) : c;
            return light;
        }
        float3 lin = gamut2020(srgb_linear(c)) * sdrWhiteScale;
        [branch] if (hdr == 2) {
            light = max(lin, 0);
            return hlg(light);
        }
        light = saturate(lin / 125.0);
        return pq(light);
    }
    float3 rgb = sample_source(target);
    [branch] if (hdr == 0) {
        light = sdr_signal(rgb);
        return light;
    }
    [branch] if (hdr == 2) {
        light = pixel == 2 ? depq(rgb) * 125.0 : max(gamut2020(rgb), 0);
        return hlg(light);
    }
    [branch] if (pixel == 2) {
        light = depq(rgb);
        return rgb;
    }
    light = saturate(gamut2020(rgb) / 125.0);
    return pq(light);
}
float3 chromaCodes(float3 light) {
    [branch] if (hdr == 0) return light;
    [branch] if (hdr == 2) return hlg(light);
    return pq(light);
}
float3 leftChroma(float2 at, bool menu, float3 l00, float3 l10, float3 l01, float3 l11) {
    float2 left = float2(max(at.x - 1, 0), at.y);
    return chromaCodes(((chroma_sample_at(left, menu) + chroma_sample_at(left + float2(0, 1), menu)) +
                        2 * (l00 + l01) + l10 + l11) * 0.125);
}
float lumaPixel(float2 p, bool menu) {
    if (menu) menu_clip(uint2(p));
    return lumaCode(rgb_at(p - 0.5, menu));
}
float2 chromaPixel(float2 p, bool menu) {
    float2 at = floor(p) * 2;
    if (menu) menu_clip(uint2(at));
    return chromaCode(leftChroma(at, menu, chroma_sample_at(at, menu), chroma_sample_at(at + float2(1, 0), menu),
                                 chroma_sample_at(at + float2(0, 1), menu), chroma_sample_at(at + float2(1, 1), menu)));
}
float4 luma(float4 p : SV_Position) : SV_Target { return float4(lumaPixel(p.xy, false), 0, 0, 1); }
float4 chroma(float4 p : SV_Position) : SV_Target { return float4(chromaPixel(p.xy, false), 0, 1); }
float4 menu_luma(float4 p : SV_Position) : SV_Target { return float4(lumaPixel(p.xy, true), 0, 0, 1); }
float4 menu_chroma(float4 p : SV_Position) : SV_Target { return float4(chromaPixel(p.xy, true), 0, 1); }
// The same conversion on a compute queue, which keeps running while a game
// fills the graphics queue. The two views address the luma and chroma planes.
RWTexture2D<float> lumaPlane : register(u0);
RWTexture2D<float2> chromaPlane : register(u1);
// One thread per 2x2 block: reuse its four luma samples for chroma, plus
// the two samples to the left needed for the co-sited horizontal filter.
// Returns the chroma it wrote, and the luma in `y` (00, 10, 01, 11).
float2 yuv420Block(uint2 id, bool menu, out float4 y) {
    uint2 at = id * 2;
    float3 l00, l10, l01, l11;
    // Each store follows its code, keeping live temporaries minimal for fxc.
    y.x = lumaCode(pixel_and_light(float2(at), menu, l00));
    lumaPlane[at] = y.x;
    y.y = lumaCode(pixel_and_light(float2(at + uint2(1, 0)), menu, l10));
    lumaPlane[at + uint2(1, 0)] = y.y;
    y.z = lumaCode(pixel_and_light(float2(at + uint2(0, 1)), menu, l01));
    lumaPlane[at + uint2(0, 1)] = y.z;
    y.w = lumaCode(pixel_and_light(float2(at + uint2(1, 1)), menu, l11));
    lumaPlane[at + uint2(1, 1)] = y.w;
    float2 uv = chromaCode(leftChroma(float2(at), menu, l00, l10, l01, l11));
    chromaPlane[id] = uv;
    return uv;
}
// Right-column chroma-light samples shared across each 8x8 threadgroup so
// `gtid.x > 0` reuses its left neighbour's right column instead of re-reading
// and re-converting two source pixels per 2x2 block.
groupshared float3 yuv420Right0[8][8];
groupshared float3 yuv420Right1[8][8];
float4 yuv420LumaAndLight(uint2 at, out float3 l00, out float3 l10, out float3 l01, out float3 l11) {
    float4 y;
    // Each store follows its code, keeping live temporaries minimal for fxc.
    y.x = lumaCode(pixel_and_light(float2(at), false, l00));
    lumaPlane[at] = y.x;
    y.y = lumaCode(pixel_and_light(float2(at + uint2(1, 0)), false, l10));
    lumaPlane[at + uint2(1, 0)] = y.y;
    y.z = lumaCode(pixel_and_light(float2(at + uint2(0, 1)), false, l01));
    lumaPlane[at + uint2(0, 1)] = y.z;
    y.w = lumaCode(pixel_and_light(float2(at + uint2(1, 1)), false, l11));
    lumaPlane[at + uint2(1, 1)] = y.w;
    return y;
}
[numthreads(8, 8, 1)]
void yuv420_cs(uint3 id : SV_DispatchThreadID, uint3 gtid : SV_GroupThreadID) {
    bool active = all(id.xy < targetSize / 2);
    uint2 at = id.xy * 2;
    float3 l00 = 0, l10 = 0, l01 = 0, l11 = 0;
    if (active) {
        yuv420LumaAndLight(at, l00, l10, l01, l11);
        yuv420Right0[gtid.y][gtid.x] = l10;
        yuv420Right1[gtid.y][gtid.x] = l11;
    }
    GroupMemoryBarrierWithGroupSync();
    if (active) {
        float3 left0;
        float3 left1;
        if (gtid.x > 0) {
            left0 = yuv420Right0[gtid.y][gtid.x - 1];
            left1 = yuv420Right1[gtid.y][gtid.x - 1];
        } else if (at.x == 0) {
            left0 = l00;
            left1 = l01;
        } else {
            float2 left = float2(at.x - 1, at.y);
            left0 = chroma_sample_at(left, false);
            left1 = chroma_sample_at(left + float2(0, 1), false);
        }
        chromaPlane[id.xy] = chromaCode(chromaCodes((left0 + left1 + 2 * (l00 + l01) + l10 + l11) * 0.125));
    }
}
// The menu passes run over the panel's blocks only, `menuBlocks` of the
// entry's own block size from `menuOriginBlocks`, after the closed pass.
[numthreads(8, 8, 1)]
void yuv420_menu_cs(uint3 id : SV_DispatchThreadID) {
    if (any(id.xy >= menuBlocks)) return;
    float4 y;
    yuv420Block(menuOriginBlocks + id.xy, true, y);
}
// Version 1 is a permutation of full-resolution quantized Y/U/V. Padding
// extends the logical edge before conversion, without a chroma filter.
uint2 packedSize() { return (targetSize + uint2(63, 7)) & ~uint2(63, 7); }
float3 packedRgb(uint2 at, bool menu) { return rgb_at(float2(min(at, targetSize - 1)), menu); }
// The extra chroma regions are signed half-differences in version 2. Keeping
// their full range avoids modular wraparound when the codec quantizes them.
float2 packedDetail(float2 value, float2 base) {
    float unit = tenBit != 0 ? 65535.0 / 64 : 255;
    float maximum = tenBit != 0 ? 1023 : 255;
    float center = tenBit != 0 ? 512 : 128;
    return clamp(floor((round(value * unit) - round(base * unit)) * 0.5 + center + 0.5), 0, maximum) / unit;
}
float packedY(uint2 at, bool menu) {
    uint2 size = packedSize();
    if (at.y < size.y) {
        if (menu) menu_clip(at);
        return lumaCode(packedRgb(at, menu));
    }
    uint plane = at.x >= size.x / 2 ? 1 : 0;
    uint2 pixel = uint2((at.x % (size.x / 2)) * 2 + 1, at.y - size.y);
    if (menu) menu_clip(pixel);
    float2 uv = chromaCode(packedRgb(pixel, menu));
    if (padding == 2) uv = packedDetail(uv, chromaCode(packedRgb(pixel & ~uint2(1, 1), menu)));
    return plane == 0 ? uv.x : uv.y;
}
// Version 2's two halves of the chroma plane: anchors, then the lower
// left's half-differences. The entries keep the version test and its early
// return, which fxc compiles to `ret` there but not in an inlined function.
float2 packedUv2(float2 p, bool menu) {
    uint2 size = packedSize();
    uint2 at = uint2(uint(p.x) * 2, (uint(p.y) % (size.y / 2)) * 2);
    if (menu) menu_clip(at);
    float2 base = chromaCode(packedRgb(at, menu));
    return uint(p.y) < size.y / 2 ? base : packedDetail(chromaCode(packedRgb(at + uint2(0, 1), menu)), base);
}
float2 packedUv1(float2 p, bool menu) {
    uint2 at = uint2(uint(p.x) * 2, uint(p.y));
    if (menu) menu_clip(at);
    return chromaCode(packedRgb(at, menu));
}
float4 chroma_pack_y(float4 p : SV_Position) : SV_Target { return float4(packedY(uint2(p.xy), false), 0, 0, 1); }
float4 chroma_pack_uv(float4 p : SV_Position) : SV_Target {
    if (padding == 2) return float4(packedUv2(p.xy, false), 0, 1);
    return float4(packedUv1(p.xy, false), 0, 1);
}
float4 menu_chroma_pack_y(float4 p : SV_Position) : SV_Target { return float4(packedY(uint2(p.xy), true), 0, 0, 1); }
float4 menu_chroma_pack_uv(float4 p : SV_Position) : SV_Target {
    if (padding == 2) return float4(packedUv2(p.xy, true), 0, 1);
    return float4(packedUv1(p.xy, true), 0, 1);
}
// Version 2: one thread owns one complete 2x2 block, `id` in blocks, and
// reuses the anchor for all three enhancement samples.
void packedBlock(uint2 id, uint2 size, bool menu) {
    // `id * 2`, written as `chroma_pack_cs` always did: fxc allocates
    // registers by source shape, and this keeps its bytecode unchanged.
    uint2 at = uint2(id.x * 2, id.y);
    at.y = id.y * 2;
    float3 even = packedRgb(at, menu);
    float3 odd = packedRgb(at + uint2(1, 0), menu);
    float3 bottomLeft = packedRgb(at + uint2(0, 1), menu);
    float3 bottomRight = packedRgb(at + uint2(1, 1), menu);
    float2 base = chromaCode(even);
    float2 topDetail = packedDetail(chromaCode(odd), base);
    float2 bottomDetail = packedDetail(chromaCode(bottomRight), base);
    lumaPlane[at] = lumaCode(even);
    lumaPlane[at + uint2(1, 0)] = lumaCode(odd);
    lumaPlane[at + uint2(0, 1)] = lumaCode(bottomLeft);
    lumaPlane[at + uint2(1, 1)] = lumaCode(bottomRight);
    lumaPlane[uint2(id.x, size.y + at.y)] = topDetail.x;
    lumaPlane[uint2(size.x / 2 + id.x, size.y + at.y)] = topDetail.y;
    lumaPlane[uint2(id.x, size.y + at.y + 1)] = bottomDetail.x;
    lumaPlane[uint2(size.x / 2 + id.x, size.y + at.y + 1)] = bottomDetail.y;
    chromaPlane[id] = base;
    chromaPlane[uint2(id.x, size.y / 2 + id.y)] = packedDetail(chromaCode(bottomLeft), base);
}
// Version 1: one thread per 2x1 pair, `id` in pairs, whose two pixels the
// entry reads before its version test, as `chroma_pack_cs` always did.
void packedPair(uint2 id, uint2 size, float3 even, float3 odd) {
    uint2 at = uint2(id.x * 2, id.y);
    float2 uv = chromaCode(odd);
    lumaPlane[at] = lumaCode(even);
    lumaPlane[at + uint2(1, 0)] = lumaCode(odd);
    lumaPlane[uint2(id.x, size.y + id.y)] = uv.x;
    lumaPlane[uint2(size.x / 2 + id.x, size.y + id.y)] = uv.y;
    chromaPlane[id] = chromaCode(even);
}
[numthreads(8, 8, 1)]
void chroma_pack_cs(uint3 id : SV_DispatchThreadID) {
    uint2 size = packedSize();
    [branch] if (padding == 2) {
        if (any(id.xy >= size / 2)) return;
        packedBlock(id.xy, size, false);
        return;
    }
    if (any(id.xy >= uint2(size.x / 2, size.y))) return;
    uint2 at = uint2(id.x * 2, id.y);
    packedPair(id.xy, size, packedRgb(at, false), packedRgb(at + uint2(1, 0), false));
}
[numthreads(8, 8, 1)]
void chroma_pack_menu_cs(uint3 id : SV_DispatchThreadID) {
    if (any(id.xy >= menuBlocks)) return;
    uint2 block = menuOriginBlocks + id.xy;
    [branch] if (padding == 2) {
        packedBlock(block, packedSize(), true);
        return;
    }
    uint2 at = uint2(block.x * 2, block.y);
    packedPair(block, packedSize(), packedRgb(at, true), packedRgb(at + uint2(1, 0), true));
}
// PyroWave's planes hold codes over their full range (code / 1023 or 255),
// one plane each. Shared by the graphics and compute conversions.
float pyroLuma(float3 rgb) {
    float y = dot(rgb, weights());
    float maximum = tenBit != 0 ? 1023 : 255;
    float code = tenBit != 0 ? (fullRange != 0 ? 1023*y : 64+876*y) : (fullRange != 0 ? 255*y : 16+219*y);
    return floor(clamp(code,0,maximum)+0.5)/maximum;
}
float2 pyroChroma(float3 rgb) {
    float3 k = weights(); float y = dot(rgb,k);
    float2 uv = float2((rgb.b-y)/(2*(1-k.b)),(rgb.r-y)/(2*(1-k.r)));
    float maximum = tenBit != 0 ? 1023 : 255;
    float2 code = tenBit != 0 ? 512+(fullRange != 0 ? 1023 : 896)*uv : 128+(fullRange != 0 ? 255 : 224)*uv;
    return floor(clamp(code,0,maximum)+0.5)/maximum;
}
// PyroWave's decoder explicitly uses centre-sited 4:2:0 (pyrowave.h).
// Keep its 2x2 average separate from standard codecs' left-sited filter.
float2 pyroBlock(float3 l00, float3 l10, float3 l01, float3 l11) {
    // In light for HDR, as `leftChroma`.
    return pyroChroma(chromaCodes((l00 + l10 + l01 + l11) * 0.25));
}
float pyroLumaPixel(float2 p, bool menu) {
    if (menu) menu_clip(uint2(p));
    return pyroLuma(rgb_at(p - 0.5, menu));
}
float2 pyro_chroma(float2 p, bool menu) {
    if (padding != 0) {
        if (menu) menu_clip(uint2(p));
        return pyroChroma(rgb_at(p - 0.5, menu));
    }
    float2 at = floor(p)*2;
    if (menu) menu_clip(uint2(at));
    return pyroBlock(chroma_sample_at(at, menu), chroma_sample_at(at+float2(1,0), menu), chroma_sample_at(at+float2(0,1), menu), chroma_sample_at(at+float2(1,1), menu));
}
float4 pyro_y(float4 p : SV_Position) : SV_Target { return float4(pyroLumaPixel(p.xy, false),0,0,1); }
float4 pyro_u(float4 p : SV_Position) : SV_Target { return float4(pyro_chroma(p.xy, false).x,0,0,1); }
float4 pyro_v(float4 p : SV_Position) : SV_Target { return float4(pyro_chroma(p.xy, false).y,0,0,1); }
float4 menu_pyro_y(float4 p : SV_Position) : SV_Target { return float4(pyroLumaPixel(p.xy, true),0,0,1); }
float4 menu_pyro_u(float4 p : SV_Position) : SV_Target { return float4(pyro_chroma(p.xy, true).x,0,0,1); }
float4 menu_pyro_v(float4 p : SV_Position) : SV_Target { return float4(pyro_chroma(p.xy, true).y,0,0,1); }
struct Pyro444 {
    float y : SV_Target0;
    float u : SV_Target1;
    float v : SV_Target2;
};
Pyro444 pyro444Pixel(float2 p, bool menu) {
    if (menu) menu_clip(uint2(p));
    float3 rgb = rgb_at(p - 0.5, menu);
    float2 uv = pyroChroma(rgb);
    Pyro444 result;
    result.y = pyroLuma(rgb);
    result.u = uv.x;
    result.v = uv.y;
    return result;
}
Pyro444 pyro_444(float4 p : SV_Position) { return pyro444Pixel(p.xy, false); }
Pyro444 menu_pyro_444(float4 p : SV_Position) { return pyro444Pixel(p.xy, true); }
// PyroWave's three planes on the compute queue, in one pass over 2x2 blocks,
// beside a game that fills the graphics queue.
RWTexture2D<float> pyroY : register(u2);
RWTexture2D<float> pyroU : register(u3);
RWTexture2D<float> pyroV : register(u4);
void pyroBlocks(uint2 id, bool menu) {
    uint2 at = id * 2;
    float3 l00, l10, l01, l11;
    float3 c00 = pixel_and_light(float2(at), menu, l00);
    float3 c10 = pixel_and_light(float2(at + uint2(1, 0)), menu, l10);
    float3 c01 = pixel_and_light(float2(at + uint2(0, 1)), menu, l01);
    float3 c11 = pixel_and_light(float2(at + uint2(1, 1)), menu, l11);
    pyroY[at] = pyroLuma(c00);
    pyroY[at + uint2(1, 0)] = pyroLuma(c10);
    pyroY[at + uint2(0, 1)] = pyroLuma(c01);
    pyroY[at + uint2(1, 1)] = pyroLuma(c11);
    [branch] if (padding != 0) {
        float2 c = pyroChroma(c00); pyroU[at] = c.x; pyroV[at] = c.y;
        c = pyroChroma(c10); pyroU[at + uint2(1, 0)] = c.x; pyroV[at + uint2(1, 0)] = c.y;
        c = pyroChroma(c01); pyroU[at + uint2(0, 1)] = c.x; pyroV[at + uint2(0, 1)] = c.y;
        c = pyroChroma(c11); pyroU[at + uint2(1, 1)] = c.x; pyroV[at + uint2(1, 1)] = c.y;
    } else {
        float2 c = pyroBlock(l00, l10, l01, l11);
        pyroU[id] = c.x;
        pyroV[id] = c.y;
    }
}
[numthreads(8, 8, 1)]
void pyro_cs(uint3 id : SV_DispatchThreadID) {
    if (any(id.xy >= targetSize / 2)) return;
    pyroBlocks(id.xy, false);
}
[numthreads(8, 8, 1)]
void pyro_menu_cs(uint3 id : SV_DispatchThreadID) {
    if (any(id.xy >= menuBlocks)) return;
    pyroBlocks(menuOriginBlocks + id.xy, true);
}
// NVENC's native 4:4:4 input: every pixel's own chroma, unfiltered, with the
// matrix, range and rounding of `lumaCode` and `chromaCode`, so a 4:4:4
// stream codes the colours a 4:2:0 one does. The layouts are ported from
// LizardByte Sunshine's src_assets/windows/assets/shaders/directx/
// include/convert_yuv444_ps_base.hlsl and convert_yuv444_planar_vs.hlsl
// (ec2676b60bb8220a1807fc761bc2157e5e6dba63), GPL-3.0: 8-bit AYUV through
// an R8G8B8A8_UINT view (V, U, Y, A in R, G, B, A), and 10-bit as three
// R16_UINT planes stacked in one texture (Y, then U, then V, each code in
// the upper ten bits as P010 stores it), which the CUDA copy hands NVENC as
// YUV444_10BIT (`nvenc/cuda.rs`). Sunshine draws the planes through three
// viewports; here one viewport three pictures tall covers them and each
// pixel finds its plane from its row.
uint3 yuv444Codes(float3 rgb) {
    // `lumaCode` and `chromaCode` return what a UNORM view stores: code / 255,
    // or code * 64 / 65535 for ten bits. A UINT view takes the integer.
    return uint3(round(float3(lumaCode(rgb), chromaCode(rgb)) * (tenBit != 0 ? 65535.0 : 255.0)));
}
uint4 yuv444Ayuv(float2 p, bool menu) {
    if (menu) menu_clip(uint2(p));
    uint3 c = yuv444Codes(rgb_at(p - 0.5, menu));
    return uint4(c.z, c.y, c.x, 0);
}
uint yuv444Plane(float2 p, bool menu) {
    uint plane = min(uint(p.y) / targetSize.y, 2u);
    float2 at = float2(p.x, p.y - plane * targetSize.y);
    if (menu) menu_clip(uint2(at));
    uint3 c = yuv444Codes(rgb_at(at - 0.5, menu));
    return plane == 0 ? c.x : plane == 1 ? c.y : c.z;
}
uint4 yuv444_ayuv(float4 p : SV_Position) : SV_Target { return yuv444Ayuv(p.xy, false); }
uint4 menu_yuv444_ayuv(float4 p : SV_Position) : SV_Target { return yuv444Ayuv(p.xy, true); }
uint yuv444_planes(float4 p : SV_Position) : SV_Target { return yuv444Plane(p.xy, false); }
uint menu_yuv444_planes(float4 p : SV_Position) : SV_Target { return yuv444Plane(p.xy, true); }
// 8-bit native 4:4:4 on the D3D12 compute queue (Rubylight #150): writes a
// DXGI_FORMAT_AYUV target through an R8G8B8A8_UINT UAV (V, U, Y, 0) with the
// same codes as `yuv444_ayuv`. Reuses u0 (never bound beside `lumaPlane`),
// keeping every non-root UAV inside feature level 11_0's u0-u7.
RWTexture2D<uint4> ayuvPlane : register(u0);
uint4 yuv444AyuvPixel(uint2 at, bool menu) {
    uint3 c = yuv444Codes(rgb_at(float2(at), menu));
    return uint4(c.z, c.y, c.x, 0);
}
[numthreads(16, 16, 1)]
void yuv444_ayuv_cs(uint3 id : SV_DispatchThreadID) {
    if (any(id.xy >= targetSize)) return;
    ayuvPlane[id.xy] = yuv444AyuvPixel(id.xy, false);
}
[numthreads(8, 8, 1)]
void yuv444_ayuv_menu_cs(uint3 id : SV_DispatchThreadID) {
    if (any(id.xy >= menuBlocks)) return;
    uint2 at = menuOriginBlocks + id.xy;
    ayuvPlane[at] = yuv444AyuvPixel(at, true);
}
// 10-bit native 4:4:4 in a single compute pass: converts each pixel once and
// writes Y, U and V into the stacked R16_UINT target (`Layout::Planes444`),
// reusing u0 (never bound beside `lumaPlane` or `ayuvPlane`).
RWTexture2D<uint> yuv444Planes : register(u0);
void yuv444PlanesPixel(uint2 at, bool menu) {
    uint3 c = yuv444Codes(rgb_at(float2(at), menu));
    yuv444Planes[at] = c.x;
    yuv444Planes[uint2(at.x, at.y + targetSize.y)] = c.y;
    yuv444Planes[uint2(at.x, at.y + 2 * targetSize.y)] = c.z;
}
[numthreads(16, 16, 1)]
void yuv444_planes_cs(uint3 id : SV_DispatchThreadID) {
    if (any(id.xy >= targetSize)) return;
    yuv444PlanesPixel(id.xy, false);
}
[numthreads(8, 8, 1)]
void yuv444_planes_menu_cs(uint3 id : SV_DispatchThreadID) {
    if (any(id.xy >= menuBlocks)) return;
    yuv444PlanesPixel(menuOriginBlocks + id.xy, true);
}
// Encoder focus (docs/encoder-focus-design.md): `yuv420_cs` plus a still-age
// per 16x16 block, then the codec blocks' importances AMF reads as its ROI
// map. Kept apart so the focus-off pipeline stays the shader it was.
cbuffer Focus : register(b1) {
    uint focusMode, focusMoving, focusStill, focusRamp;
    uint focusCentre, focusCtbStill, focusNoiseBits, focusBlock;
    uint2 focusAgeGrid, focusGrid;
    int2 focusCursor;
    uint focusCursorVisible, focusReset;
};
// {signature, age} per 16x16 block; the map is {min, max} then the importances.
RWStructuredBuffer<uint2> focusAges : register(u5);
RWStructuredBuffer<uint> focusMap : register(u6);
groupshared uint focusSum;
uint focusHash(uint v) {
    uint state = v * 747796405u + 2891336453u;
    uint word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}
// The integer code `lumaCode` or `chromaCode` wrote, less the ignored bits.
uint focusCode(float v) {
    return (tenBit != 0 ? uint(v * (65535.0 / 64.0) + 0.5) : uint(v * 255.0 + 0.5)) >> focusNoiseBits;
}
[numthreads(8, 8, 1)]
void yuv420_focus_cs(uint3 id : SV_DispatchThreadID, uint3 group : SV_GroupID, uint3 gtid : SV_GroupThreadID, uint index : SV_GroupIndex) {
    if (index == 0) {
        focusSum = 0;
        if (all(group.xy == 0)) {
            focusMap[0] = 0xffffffff;
            focusMap[1] = 0;
        }
    }
    bool active = all(id.xy < targetSize / 2);
    uint2 at = id.xy * 2;
    float4 y = 0;
    float3 l00 = 0, l10 = 0, l01 = 0, l11 = 0;
    if (active) {
        y = yuv420LumaAndLight(at, l00, l10, l01, l11);
        yuv420Right0[gtid.y][gtid.x] = l10;
        yuv420Right1[gtid.y][gtid.x] = l11;
    }
    GroupMemoryBarrierWithGroupSync();
    if (active) {
        float3 left0;
        float3 left1;
        if (gtid.x > 0) {
            left0 = yuv420Right0[gtid.y][gtid.x - 1];
            left1 = yuv420Right1[gtid.y][gtid.x - 1];
        } else if (at.x == 0) {
            left0 = l00;
            left1 = l01;
        } else {
            float2 left = float2(at.x - 1, at.y);
            left0 = chroma_sample_at(left, false);
            left1 = chroma_sample_at(left + float2(0, 1), false);
        }
        float2 uv = chromaCode(chromaCodes((left0 + left1 + 2 * (l00 + l01) + l10 + l11) * 0.125));
        chromaPlane[id.xy] = uv;
        uint h = focusHash(index * 0x9e3779b9u ^ focusCode(y.x));
        h = focusHash(h ^ focusCode(y.y));
        h = focusHash(h ^ focusCode(y.z));
        h = focusHash(h ^ focusCode(y.w));
        h = focusHash(h ^ focusCode(uv.x));
        InterlockedAdd(focusSum, focusHash(h ^ focusCode(uv.y)));
    }
    GroupMemoryBarrierWithGroupSync();
    if (index == 0) {
        uint ageAt = group.y * focusAgeGrid.x + group.x;
        uint2 previous = focusAges[ageAt];
        bool same = focusReset == 0 && previous.x == focusSum;
        focusAges[ageAt] = uint2(focusSum, same ? min(previous.y + 1, 255) : 0);
    }
}
// Encoder focus for 8-bit native 4:4:4: one 16x16 thread group per 16x16 age
// block, matching `yuv420_focus_cs`'s `focusAgeGrid` and `focus_map_cs`.
[numthreads(16, 16, 1)]
void yuv444_ayuv_focus_cs(uint3 id : SV_DispatchThreadID, uint3 group : SV_GroupID, uint index : SV_GroupIndex) {
    if (index == 0) {
        focusSum = 0;
        if (all(group.xy == 0)) {
            focusMap[0] = 0xffffffff;
            focusMap[1] = 0;
        }
    }
    GroupMemoryBarrierWithGroupSync();
    if (all(id.xy < targetSize)) {
        uint4 p = yuv444AyuvPixel(id.xy, false);
        ayuvPlane[id.xy] = p;
        // `p` holds (V, U, Y, 0) as 8-bit integer codes.
        uint h = focusHash(index * 0x9e3779b9u ^ (p.z >> focusNoiseBits));
        h = focusHash(h ^ (p.y >> focusNoiseBits));
        InterlockedAdd(focusSum, focusHash(h ^ (p.x >> focusNoiseBits)));
    }
    GroupMemoryBarrierWithGroupSync();
    if (index == 0) {
        uint at = group.y * focusAgeGrid.x + group.x;
        uint2 previous = focusAges[at];
        bool same = focusReset == 0 && previous.x == focusSum;
        focusAges[at] = uint2(focusSum, same ? min(previous.y + 1, 255) : 0);
    }
}
// A 64x64 block's age is the k-th largest of its 16x16 blocks' ages, so it
// counts as still once `focusCtbStill`/256 of them are (`encoder_focus::ctb_age`).
uint focusBlockAge(uint2 block) {
    if (focusBlock == 16) return focusAges[block.y * focusAgeGrid.x + block.x].y;
    uint2 first = block * 4;
    uint2 last = min(first + 4, focusAgeGrid);
    uint ages[16];
    uint n = 0;
    for (uint y = first.y; y < last.y; y++)
        for (uint x = first.x; x < last.x; x++) ages[n++] = focusAges[y * focusAgeGrid.x + x].y;
    uint k = max(1, (min(focusCtbStill, 256) * n + 255) / 256);
    uint age = 0;
    for (uint i = 0; i < n; i++) {
        uint above = 0;
        for (uint j = 0; j < n; j++) above += ages[j] >= ages[i] ? 1 : 0;
        if (above >= k) age = max(age, ages[i]);
    }
    return age;
}
// `encoder_focus::centre_weight`, in the same integer steps.
uint focusCentreWeight(uint2 block) {
    int2 c = int2(block * focusBlock + focusBlock / 2);
    uint2 n = min(min(uint2(abs(2 * c - int2(targetSize))), 65535) * 1024 / max(targetSize / 2, 1), 2048);
    if (n.x * n.x + n.y * n.y <= 1024 * 1024) return 256;
    if (focusCursorVisible != 0) {
        uint2 d = min(uint2(abs(c - focusCursor)), 32767);
        if (d.x * d.x + d.y * d.y <= 128 * 128) return 256;
    }
    return 0;
}
groupshared uint focusGroupMin;
groupshared uint focusGroupMax;
// One thread per codec block: `encoder_focus::importance`.
[numthreads(8, 8, 1)]
void focus_map_cs(uint3 id : SV_DispatchThreadID, uint index : SV_GroupIndex) {
    if (index == 0) {
        focusGroupMin = 0xffffffff;
        focusGroupMax = 0;
    }
    GroupMemoryBarrierWithGroupSync();
    if (all(id.xy < focusGrid)) {
        int ramp = int(clamp(focusRamp, 1, 255));
        int moving = int(min(focusMoving, 10));
        int still = int(min(focusStill, 10));
        int age = int(min(focusBlockAge(id.xy), uint(ramp)));
        uint value = uint(clamp(moving + (still - moving) * age / ramp, 0, 10));
        if (focusMode == 1) value = min(value + (min(focusCentre, 10) * focusCentreWeight(id.xy) + 128) / 256, 10);
        focusMap[2 + id.y * focusGrid.x + id.x] = value;
        InterlockedMin(focusGroupMin, value);
        InterlockedMax(focusGroupMax, value);
    }
    GroupMemoryBarrierWithGroupSync();
    if (index == 0 && focusGroupMin <= focusGroupMax) {
        InterlockedMin(focusMap[0], focusGroupMin);
        InterlockedMax(focusMap[1], focusGroupMax);
    }
}
// HDR dynamic metadata statistics (butterpollo_core::hdr_dynamic): PQ(maxRGB)
// of the source as the conversion loads it (pointer drawn, before scaling and
// the couch menu), at the centres of at most 240x135 cells, in 1024 equal PQ
// bins, with the samples' minimum and
// maximum. A groupshared histogram per tile, folded into the frame's with one
// atomic per occupied bin, after Foundation Sunshine's
// hdr_luminance_analysis_cs.hlsl (AlkaidLab/foundation-sunshine, commit
// 880f1ef). Copyright (c) Foundation Sunshine authors, GPL-3.0.
cbuffer Stats : register(b3) {
    uint2 statsGrid;
};
// {min, max} as float bits, then the bins.
RWStructuredBuffer<uint> stats : register(u7);
groupshared uint statsBins[1024];
groupshared uint statsMin;
groupshared uint statsMax;
[numthreads(64, 1, 1)]
void hdr_stats_clear_cs(uint3 id : SV_DispatchThreadID) {
    if (id.x < 2 + 1024) stats[id.x] = id.x == 0 ? 0xffffffff : 0;
}
[numthreads(16, 16, 1)]
void hdr_stats_cs(uint3 id : SV_DispatchThreadID, uint index : SV_GroupIndex) {
    [unroll] for (uint clear = 0; clear < 4; clear++) statsBins[index + clear * 256] = 0;
    if (index == 0) {
        statsMin = 0xffffffff;
        statsMax = 0;
    }
    GroupMemoryBarrierWithGroupSync();
    if (all(id.xy < statsGrid)) {
        float3 rgb = load(int2((2 * id.xy + 1) * sourceSize / (2 * statsGrid)));
        float peak;
        if (pixel != 2) {
            // `pq1` is monotonic, so PQ(maxRGB) needs one scalar evaluation instead of three.
            float3 lin = gamut2020(rgb) / 125.0;
            peak = pq1(max(lin.r, max(lin.g, lin.b)));
        } else {
            peak = max(rgb.r, max(rgb.g, rgb.b));
        }
        // An HLG stream is graded for 1000 nits (`hlg`): its statistics end
        // there, PQ(1000 nits), as the signal does.
        if (hdr == 2) peak = min(peak, 0.7518271);
        // Without the sign bit, the bits of floats in 0..=1 order as integers.
        uint code = asuint(saturate(peak)) & 0x7fffffff;
        InterlockedAdd(statsBins[min(uint(asfloat(code) * 1024), 1023)], 1);
        InterlockedMin(statsMin, code);
        InterlockedMax(statsMax, code);
    }
    GroupMemoryBarrierWithGroupSync();
    [unroll] for (uint fold = 0; fold < 4; fold++) {
        uint bin = index + fold * 256;
        if (statsBins[bin] != 0) InterlockedAdd(stats[2 + bin], statsBins[bin]);
    }
    if (index == 0 && statsMin <= statsMax) {
        InterlockedMin(stats[0], statsMin);
        InterlockedMax(stats[1], statsMax);
    }
}
// Frame Warp (docs/frame-warp.md, `compute_frame_warp.rs`): a luma thumbnail
// of each capture for the host's motion measurement, and the HUD, the
// textured pixels that stay the same within a small tolerance while the
// camera turns, scored per mask cell. Both read the capture as captured: before the pointer,
// scaling or the menu.
cbuffer FrameWarp : register(b4) {
    uint2 fwSource; uint2 fwThumb;
    uint2 fwGrid; uint fwPixel; uint fwMode;
    uint fwThumbAt; uint fwCodesAt; uint fwCountsAt; uint fwScoresAt;
    uint fwBitsAt; uint fwStride; uint fwThreshold; uint fwPadding;
};
// The thumbnail's words, or the HUD's codes per pixel, then its changed
// pixels, scores and mask bits per cell. D3D12 only (a root UAV), so past the
// u0-u7 a D3D11 shader at feature level 11_0 may use.
RWStructuredBuffer<uint> frameWarp : register(u8);
// scRGB as PQ of scRGB / 125: 0 to 1, like the sRGB codes of an SDR capture
// and the PQ codes of an HDR10 capture, which are taken as captured.
float3 frameWarpPq(float3 rgb) {
    return pq(max(rgb, 0) / 125.0);
}
// The luma of a captured pixel's channels, 0 to 1 each (after frameWarpPq for
// scRGB).
float frameWarpLuma(float3 channels) {
    if (fwPixel == 2) return dot(channels, float3(0.2627, 0.6780, 0.0593));
    return dot(channels, float3(0.2126, 0.7152, 0.0722));
}
// Four thumbnail pixels a thread, a byte each in one word, rows top to bottom:
// the mean of at most 8x8 samples spread over each pixel's box of the source.
// The sample rows are worked out once a thread and the columns once a box, a
// row's eight loads go out together, and the sum takes the samples in the
// same order as one at a time would.
[numthreads(8, 8, 1)]
void frame_warp_thumb_cs(uint3 id : SV_DispatchThreadID) {
    uint words = fwThumb.x / 4;
    if (id.x >= words || id.y >= fwThumb.y) return;
    uint y0 = id.y * fwSource.y / fwThumb.y;
    uint spanY = max((id.y + 1) * fwSource.y / fwThumb.y, y0 + 1) - y0;
    uint ny = min(spanY, 8u);
    uint ys[8];
    [unroll] for (uint r = 0; r < 8; r++) ys[r] = y0 + (2 * r + 1) * spanY / (2 * ny);
    uint quad = 0;
    [loop] for (uint i = 0; i < 4; i++) {
        uint x0 = (id.x * 4 + i) * fwSource.x / fwThumb.x;
        uint spanX = max((id.x * 4 + i + 1) * fwSource.x / fwThumb.x, x0 + 1) - x0;
        uint nx = min(spanX, 8u);
        uint xs[8];
        [unroll] for (uint c = 0; c < 8; c++) xs[c] = x0 + (2 * c + 1) * spanX / (2 * nx);
        float sum = 0;
        [unroll] for (uint sy = 0; sy < 8; sy++) {
            if (sy < ny) {
                // Columns from nx on are loaded too but not summed: at most 7
                // pixels past the box, and a load outside the capture reads 0.
                float3 row[8];
                [unroll] for (uint lx = 0; lx < 8; lx++) row[lx] = source.Load(int3(xs[lx], ys[sy], 0)).rgb;
                [branch] if (fwPixel == 1) {
                    [unroll] for (uint sx = 0; sx < 8; sx++) {
                        [flatten] if (sx < nx) sum += frameWarpLuma(frameWarpPq(row[sx]));
                    }
                } else {
                    [unroll] for (uint sx = 0; sx < 8; sx++) {
                        [flatten] if (sx < nx) sum += frameWarpLuma(row[sx]);
                    }
                }
            }
        }
        quad |= uint(saturate(sum / float(nx * ny)) * 255.0 + 0.5) << (8 * i);
    }
    frameWarp[fwThumbAt + id.y * words + id.x] = quad;
}
// How far, in 10-bit steps, a channel may move between passes with its pixel
// still counted as the same: 8 is 2/255. Games add faint noise or dither to
// their HUD; in Assassin's Creed Black Flag Resynced, while the camera turned,
// the static compass bar was 99% within 2/255 between frames but only about
// 44% bit-identical, and the quest text 89% within 2/255 but about 50%
// identical, so exact codes never let those cells reach a HUD score.
static const uint fwTolerance = 8;
// A captured pixel's channels (after frameWarpPq for scRGB), 10 bits each in
// one word (R10G10B10). 8-bit codes land on every fourth step, so 2/255 is
// exactly fwTolerance; 10-bit codes land on every step.
uint frameWarpCode(float3 channels) {
    uint3 steps = uint3(saturate(channels) * (fwPixel == 0 ? 1020.0 : 1023.0) + 0.5);
    return steps.r | (steps.g << 10) | (steps.b << 20);
}
// Whether any channel of two codes differs by more than fwTolerance.
bool frameWarpChanged(uint a, uint b) {
    uint3 x = uint3(a, a >> 10, a >> 20) & 1023;
    uint3 y = uint3(b, b >> 10, b >> 20) & 1023;
    return any(max(x, y) - min(x, y) > fwTolerance);
}
// Whether the step from code a to its neighbour b is the step from c to d,
// the same two pixels in the last pass: every channel within fwTolerance or
// half the step.
bool frameWarpSameStep(uint a, uint b, uint c, uint d) {
    int3 now = int3(uint3(a, a >> 10, a >> 20) & 1023) - int3(uint3(b, b >> 10, b >> 20) & 1023);
    int3 then = int3(uint3(c, c >> 10, c >> 20) & 1023) - int3(uint3(d, d >> 10, d >> 20) & 1023);
    return all(abs(now - then) <= max(int(fwTolerance), abs(now) / 2));
}
// Counts per HUD cell the pixels that changed since the last pass, the
// textured ones (a code that differs by more than fwTolerance from its right
// or lower neighbour, its left or upper one at the tile's or capture's edge)
// and the textured ones that changed, and keeps every pixel's code for the
// next pass. A pixel textured in both passes whose steps to those neighbours
// stayed the same counts as unchanged: the edge of a translucent HUD, grey
// text over a moving scene, keeps its place and its step while its value
// follows what is behind it. A group takes a 16x16 tile, a thread four pixels
// of one of its rows. A cell is no smaller than a pixel, so a tile spans at
// most 17x17 cells: the tile's cell per column and per row are worked out
// once a group, and a thread adds its pixels once per cell they fall in rather
// than once per pixel.
groupshared uint fwTileColumns[16];
groupshared uint fwTileRows[16];
groupshared uint fwTileCodes[16 * 16];
groupshared uint fwTileBefore[16 * 16];
groupshared uint fwTileCells[3 * 17 * 17];
void frameWarpAddRun(uint cell, uint3 run) {
    if (run.x != 0) InterlockedAdd(fwTileCells[3 * cell], run.x);
    if (run.y != 0) InterlockedAdd(fwTileCells[3 * cell + 1], run.y);
    if (run.z != 0) InterlockedAdd(fwTileCells[3 * cell + 2], run.z);
}
[numthreads(4, 16, 1)]
void frame_warp_hud_cs(uint3 group : SV_GroupID, uint3 inGroup : SV_GroupThreadID, uint index : SV_GroupIndex) {
    uint2 tile = group.xy * 16;
    uint2 first = tile * fwGrid / fwSource;
    for (uint c = index; c < 3 * 17 * 17; c += 64) fwTileCells[c] = 0;
    if (index < 16) fwTileColumns[index] = (tile.x + index) * fwGrid.x / fwSource.x - first.x;
    else if (index < 32) fwTileRows[index - 16] = (tile.y + index - 16) * fwGrid.y / fwSource.y - first.y;
    // Bit k: pixel x0 + k changed, or is textured.
    uint changed = 0;
    uint textured = 0;
    uint4 codes = 0;
    uint4 before = 0;
    uint x0 = tile.x + inGroup.x * 4;
    uint y = tile.y + inGroup.y;
    if (y < fwSource.y) {
        uint row = fwCodesAt + y * fwSource.x;
        // The four pixels' loads go out together. Past the capture's right
        // edge the texture reads 0 and the code is the row's last, never past
        // the buffer; neither is used.
        float3 rgb[4];
        [unroll] for (uint k = 0; k < 4; k++) {
            rgb[k] = source.Load(int3(x0 + k, y, 0)).rgb;
            before[k] = frameWarp[row + min(x0 + k, fwSource.x - 1)];
        }
        [branch] if (fwPixel == 1) {
            [unroll] for (uint p = 0; p < 4; p++) codes[p] = frameWarpCode(frameWarpPq(rgb[p]));
        } else {
            [unroll] for (uint p = 0; p < 4; p++) codes[p] = frameWarpCode(rgb[p]);
        }
        [unroll] for (uint q = 0; q < 4; q++) {
            if (x0 + q < fwSource.x) {
                if (frameWarpChanged(before[q], codes[q])) changed |= 1u << q;
                frameWarp[row + x0 + q] = codes[q];
            }
        }
    }
    [unroll] for (uint s = 0; s < 4; s++) {
        fwTileCodes[inGroup.y * 16 + inGroup.x * 4 + s] = codes[s];
        fwTileBefore[inGroup.y * 16 + inGroup.x * 4 + s] = before[s];
    }
    GroupMemoryBarrierWithGroupSync();
    if (y < fwSource.y) {
        uint ly = inGroup.y;
        bool below = ly < 15 && y + 1 < fwSource.y;
        // A pixel with no neighbour (a capture one pixel wide or high) is its
        // own, which never differs.
        uint other = below ? ly + 1 : (ly > 0 ? ly - 1 : ly);
        [unroll] for (uint q = 0; q < 4; q++) {
            uint lx = inGroup.x * 4 + q;
            if (x0 + q < fwSource.x) {
                bool right = lx < 15 && x0 + q + 1 < fwSource.x;
                uint beside = right ? lx + 1 : (lx > 0 ? lx - 1 : lx);
                uint here = ly * 16 + lx;
                uint side = ly * 16 + beside;
                uint under = other * 16 + lx;
                uint code = fwTileCodes[here];
                uint then = fwTileBefore[here];
                if (frameWarpChanged(code, fwTileCodes[side])
                    || frameWarpChanged(code, fwTileCodes[under])) {
                    textured |= 1u << q;
                    if ((changed >> q & 1) != 0
                        && (frameWarpChanged(then, fwTileBefore[side])
                            || frameWarpChanged(then, fwTileBefore[under]))
                        && frameWarpSameStep(code, fwTileCodes[side], then, fwTileBefore[side])
                        && frameWarpSameStep(code, fwTileCodes[under], then, fwTileBefore[under])) {
                        changed &= ~(1u << q);
                    }
                }
            }
        }
    }
    uint moved = changed & textured;
    if ((changed | textured) != 0) {
        // Cells only grow along a row, so the thread's pixels fall in runs.
        uint cells = fwTileRows[inGroup.y] * 17;
        uint column = fwTileColumns[inGroup.x * 4];
        uint3 run = uint3(changed, textured, moved) & 1;
        [unroll] for (uint j = 1; j < 4; j++) {
            uint next = fwTileColumns[inGroup.x * 4 + j];
            if (next != column) {
                frameWarpAddRun(cells + column, run);
                column = next;
                run = 0;
            }
            run += (uint3(changed, textured, moved) >> j) & 1;
        }
        frameWarpAddRun(cells + column, run);
    }
    GroupMemoryBarrierWithGroupSync();
    for (uint t = index; t < 17 * 17; t += 64) {
        uint2 cell = first + uint2(t % 17, t / 17);
        if (all(cell < fwGrid)) {
            uint at = fwCountsAt + 3 * (cell.y * fwGrid.x + cell.x);
            [unroll] for (uint f = 0; f < 3; f++) {
                if (fwTileCells[3 * t + f] != 0) InterlockedAdd(frameWarp[at + f], fwTileCells[3 * t + f]);
            }
        }
    }
}
// Per HUD cell, as the camera turns: one where a quarter of the pixels or
// more changed loses half its score (a fade, a flash, the scene). Otherwise a
// cell with too few textured pixels keeps its score: a smooth sky or fog
// looks the same moved or not, so whether it stayed tells nothing, and with
// the tolerance it would otherwise become HUD and hide whatever crosses it.
// A textured cell gains a point, up to 255, when no more than a quarter of
// its textured pixels changed, and loses half otherwise. Mode 0 only clears
// the counts (the first pass after the camera stood still), 2 clears the
// scores too (a new size).
[numthreads(8, 8, 1)]
void frame_warp_score_cs(uint3 id : SV_DispatchThreadID) {
    if (any(id.xy >= fwGrid)) return;
    uint cell = id.y * fwGrid.x + id.x;
    uint at = fwCountsAt + 3 * cell;
    uint changed = frameWarp[at];
    uint textured = frameWarp[at + 1];
    uint moved = frameWarp[at + 2];
    frameWarp[at] = 0;
    frameWarp[at + 1] = 0;
    frameWarp[at + 2] = 0;
    if (fwMode == 2) frameWarp[fwScoresAt + cell] = 0;
    if (fwMode != 1) return;
    // The pixels whose `p * fwGrid / fwSource` is this cell.
    uint2 lo = (id.xy * fwSource + fwGrid - 1) / fwGrid;
    uint2 hi = ((id.xy + 1) * fwSource + fwGrid - 1) / fwGrid;
    uint total = (hi.x - lo.x) * (hi.y - lo.y);
    uint score = frameWarp[fwScoresAt + cell];
    if (changed * 4 > total) score /= 2;
    else if (textured * 8 < total) return;
    else score = moved * 4 <= textured ? min(score + 1, 255u) : score / 2;
    frameWarp[fwScoresAt + cell] = score;
}
// The mask: a bit per cell whose score reached fwThreshold, bit i of word w
// being column 32w + i, rows of fwStride words.
[numthreads(8, 8, 1)]
void frame_warp_bits_cs(uint3 id : SV_DispatchThreadID) {
    if (id.x >= fwStride || id.y >= fwGrid.y) return;
    uint word = 0;
    for (uint bit = 0; bit < 32; bit++) {
        uint x = id.x * 32 + bit;
        if (x < fwGrid.x && frameWarp[fwScoresAt + id.y * fwGrid.x + x] >= fwThreshold) word |= 1u << bit;
    }
    frameWarp[fwBitsAt + id.y * fwStride + id.x] = word;
}
// AMF's HLG input (`amf::vpe`, `hlg_prepass` in butterpollo_core::amf_vpe):
// scene-linear BT.2020 light for a 1000-nit display, the inverse reference
// OOTF at system gamma 1.2 that `hlg` applies before its OETF; AMF's video
// converter applies the OETF, scaling and the YUV conversion. After
// Foundation Sunshine's prepare_hlg_scene_cs
// (src_assets/windows/assets/shaders/directx/prepare_hlg_scene_cs.hlsl,
// AlkaidLab/foundation-sunshine PR #1216, commit 1add1ed), Copyright (c)
// Foundation Sunshine authors, GPL-3.0. u2, shared with the PyroWave planes,
// which hlg_scene_cs never binds: the D3D11 device runs at feature level 11_0,
// whose compute shaders have u0-u7 only (u9 failed CreateComputeShader).
RWTexture2D<float4> hlgScene : register(u2);
[numthreads(16, 16, 1)]
void hlg_scene_cs(uint3 id : SV_DispatchThreadID) {
    if (any(id.xy >= sourceSize)) return;
    float3 rgb = source.Load(int3(id.xy, 0)).rgb;
    float3 light = pixel == 2 ? depq(rgb) * 125.0 : gamut2020(rgb);
    float3 display = max(light, 0) * (80.0 / 1000.0);
    float y = dot(display, float3(0.2627, 0.6780, 0.0593));
    float3 scene = y > 0 ? display * pow(max(y, 1e-6), (1.0 - 1.2) / 1.2) : 0;
    hlgScene[id.xy] = float4(scene, 1);
}

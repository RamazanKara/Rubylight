#include <cassert>
#include <cstdint>
#include <initializer_list>
#include "../../main/jni/vulkan/ycbcr_components.h"

using namespace vkr;

// VkComponentSwizzle and VkComponentMapping as vulkan_core.h declares them
enum Swizzle { kIdentity = 0, kZero = 1, kOne = 2, kR = 3, kG = 4, kB = 5, kA = 6, kMaxEnum = 0x7FFFFFFF };
struct Mapping {
    Swizzle r, g, b, a;
};

constexpr int32_t kP010 = kVkFormatG10X6B10X6R10X6TwoPlane420Unorm3Pack16;
constexpr int32_t kNv12 = 1000156003;  // VK_FORMAT_G8_B8R8_2PLANE_420_UNORM
constexpr int32_t kExternal = 0;       // VK_FORMAT_UNDEFINED, with an external format
constexpr uint32_t kOtherVendors[] = {0x5143 /* Qualcomm */, 0x13B5 /* ARM */, 0x1010 /* Imagination */,
                                      0x144D /* Samsung */, 0x1002 /* AMD */};

constexpr Mapping kIdentityMapping = {kIdentity, kIdentity, kIdentity, kIdentity};

static bool same(const Mapping& a, const Mapping& b) {
    return a.r == b.r && a.g == b.g && a.b == b.b && a.a == b.a;
}

struct Rgb {
    float r, g, b;
};

// What one slot of the conversion reads from the sampled texel (VkComponentSwizzle)
static float read(Swizzle swizzle, float own, const Rgb& texel) {
    switch (swizzle) {
        case kIdentity: return own;
        case kZero: return 0.0f;
        case kOne: return 1.0f;
        case kR: return texel.r;
        case kG: return texel.g;
        case kB: return texel.b;
        default: return 1.0f;
    }
}

// The sampler's conversion as the spec runs it: the swizzle, then the BT.709 model reading Cr
// through R, Y through G and Cb through B (full range, to keep the numbers plain)
static Rgb convert(const Mapping& components, const Rgb& texel) {
    const float cr = read(components.r, texel.r, texel) - 0.5f;
    const float y = read(components.g, texel.g, texel);
    const float cb = read(components.b, texel.b, texel) - 0.5f;
    return {y + 1.5748f * cr, y - 0.1873f * cb - 0.4681f * cr, y + 1.8556f * cb};
}

static void tegraP010SwapsTheChroma() {
    const Mapping swapped = {kB, kIdentity, kR, kIdentity};
    assert(swapsChroma(kVkVendorNvidia, kP010));
    assert(same(ycbcrComponents(kIdentityMapping, kVkVendorNvidia, kP010), swapped));
    // Spelled out, identity swaps the same way; G and A are left as reported
    const Mapping spelled = {kR, kG, kB, kA};
    const Mapping spelledSwapped = {kB, kG, kR, kA};
    assert(same(ycbcrComponents(spelled, kVkVendorNvidia, kP010), spelledSwapped));
    // A driver that already reports the swap gets identity, never R reading R spelled out
    assert(same(ycbcrComponents(swapped, kVkVendorNvidia, kP010), kIdentityMapping));
}

static void everythingElseIsAsReported() {
    const Mapping reported = {kB, kIdentity, kR, kIdentity};
    for (int32_t format : {kNv12, kExternal}) {
        assert(!swapsChroma(kVkVendorNvidia, format));
        assert(same(ycbcrComponents(kIdentityMapping, kVkVendorNvidia, format), kIdentityMapping));
        assert(same(ycbcrComponents(reported, kVkVendorNvidia, format), reported));
    }
    for (uint32_t vendor : kOtherVendors) {
        assert(!swapsChroma(vendor, kP010));
        assert(same(ycbcrComponents(kIdentityMapping, vendor, kP010), kIdentityMapping));
        assert(same(ycbcrComponents(reported, vendor, kP010), reported));
    }
}

// The Shield's report (issue 7): a red taillight showed blue. Sampled as the driver reports it,
// a P010 texel has Cr where the conversion reads Cb and Cb where it reads Cr.
static void aRedTaillightStaysRed() {
    // R'G'B' 0.8, 0.1, 0.1 in BT.709: Y 0.249, Cb -0.080, Cr 0.350
    const float y = 0.2126f * 0.8f + 0.7152f * 0.1f + 0.0722f * 0.1f;
    const float cb = (0.1f - y) / 1.8556f;
    const float cr = (0.8f - y) / 1.5748f;
    const Rgb tegraTexel = {cb + 0.5f, y, cr + 0.5f};

    const Rgb asReported = convert(kIdentityMapping, tegraTexel);
    assert(asReported.b > 0.8f && asReported.r < 0.2f);

    const Rgb fixed = convert(ycbcrComponents(kIdentityMapping, kVkVendorNvidia, kP010), tegraTexel);
    assert(fixed.r > 0.79f && fixed.r < 0.81f);
    assert(fixed.g > 0.09f && fixed.g < 0.11f);
    assert(fixed.b > 0.09f && fixed.b < 0.11f);
}

int main() {
    tegraP010SwapsTheChroma();
    everythingElseIsAsReported();
    aRedTaillightStaysRed();
    return 0;
}

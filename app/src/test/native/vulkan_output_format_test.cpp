#include <cassert>
#include <cmath>
#include <initializer_list>
#include "../../main/jni/vulkan/output_format.h"

using namespace vkr;

// Compile the shaders' actual HLG curve as C++.
namespace glsl {
using std::exp;
static float max(float a, float b) { return a > b ? a : b; }
#include "../../main/jni/vulkan/shaders/hlg.glsl"
}

static void same(double actual, double expected, double tolerance) {
    assert(std::abs(actual - expected) < tolerance);
}

// VkFormat and VkColorSpaceKHR values the loaders list that the choice must pass over
constexpr int32_t kR8G8B8A8Srgb = 43;
constexpr int32_t kR16G16B16A16Sfloat = 97;
constexpr int32_t kR5G6B5 = 4;
constexpr int32_t kDisplayP3Nonlinear = 1000104001;
constexpr int32_t kExtendedSrgbLinear = 1000104002;
constexpr int32_t kPassThrough = 1000104013;

// Android 13 and 14 (swapchain.cpp, with VK_EXT_swapchain_colorspace): no HDR colour space at all
const OutputSurfaceFormat kAndroid14[] = {
        {kVkFormatR8G8B8A8Unorm, kVkColorSpaceSrgbNonlinear}, {kR8G8B8A8Srgb, kVkColorSpaceSrgbNonlinear},
        {kVkFormatR8G8B8A8Unorm, kPassThrough}, {kVkFormatR8G8B8A8Unorm, kDisplayP3Nonlinear},
        {kR5G6B5, kVkColorSpaceSrgbNonlinear}, {kR16G16B16A16Sfloat, kVkColorSpaceSrgbNonlinear},
        {kR16G16B16A16Sfloat, kExtendedSrgbLinear}, {kVkFormatA2B10G10R10Unorm, kVkColorSpaceSrgbNonlinear},
        {kVkFormatA2B10G10R10Unorm, kPassThrough}, {kVkFormatA2B10G10R10Unorm, kDisplayP3Nonlinear},
};
// Android 15: HDR10_ST2084 and HDR10_HLG for the ten-bit and FP16 formats
const OutputSurfaceFormat kAndroid15[] = {
        {kVkFormatR8G8B8A8Unorm, kVkColorSpaceSrgbNonlinear}, {kR16G16B16A16Sfloat, kVkColorSpaceHdr10St2084},
        {kR16G16B16A16Sfloat, kVkColorSpaceHdr10Hlg}, {kVkFormatA2B10G10R10Unorm, kVkColorSpaceSrgbNonlinear},
        {kVkFormatA2B10G10R10Unorm, kVkColorSpaceHdr10St2084}, {kVkFormatA2B10G10R10Unorm, kVkColorSpaceHdr10Hlg},
};
// No ten-bit format
const OutputSurfaceFormat kEightBit[] = {
        {kR8G8B8A8Srgb, kVkColorSpaceSrgbNonlinear}, {kVkFormatR8G8B8A8Unorm, kVkColorSpaceSrgbNonlinear},
};

template <size_t N>
static OutputChoice choose(const OutputSurfaceFormat (&formats)[N], bool hdr, int transfer, bool tenBit = true) {
    return vkSelectOutput(formats, N, hdr, transfer, tenBit);
}

static void android14GoesThroughTheWindowDataspace() {
    OutputChoice pq = choose(kAndroid14, true, kHdrTransferPq);
    assert(pq.format == kVkFormatA2B10G10R10Unorm && pq.colorSpace == kVkColorSpaceSrgbNonlinear);
    assert(pq.dataspaceOverride == kDataspaceBt2020Pq && !pq.toneMapToSdr);

    OutputChoice hlg = choose(kAndroid14, true, kHdrTransferHlg);
    assert(hlg.format == kVkFormatA2B10G10R10Unorm && hlg.colorSpace == kVkColorSpaceSrgbNonlinear);
    assert(hlg.dataspaceOverride == kDataspaceBt2020Hlg && !hlg.toneMapToSdr);
}

static void android15UsesTheNativeColorSpace() {
    OutputChoice pq = choose(kAndroid15, true, kHdrTransferPq);
    assert(pq.format == kVkFormatA2B10G10R10Unorm && pq.colorSpace == kVkColorSpaceHdr10St2084);
    assert(pq.dataspaceOverride == 0 && !pq.toneMapToSdr);

    OutputChoice hlg = choose(kAndroid15, true, kHdrTransferHlg);
    assert(hlg.format == kVkFormatA2B10G10R10Unorm && hlg.colorSpace == kVkColorSpaceHdr10Hlg);
    assert(hlg.dataspaceOverride == 0 && !hlg.toneMapToSdr);

    // A list with PQ alone still carries HLG through the dataspace, never as PQ
    const OutputSurfaceFormat pqOnly[] = {
            {kVkFormatA2B10G10R10Unorm, kVkColorSpaceSrgbNonlinear}, {kVkFormatA2B10G10R10Unorm, kVkColorSpaceHdr10St2084},
    };
    hlg = choose(pqOnly, true, kHdrTransferHlg);
    assert(hlg.colorSpace == kVkColorSpaceSrgbNonlinear && hlg.dataspaceOverride == kDataspaceBt2020Hlg);
}

static void withoutTenBitHdrIsToneMapped() {
    for (int transfer : {kHdrTransferPq, kHdrTransferHlg}) {
        OutputChoice choice = choose(kEightBit, true, transfer);
        assert(choice.format == kVkFormatR8G8B8A8Unorm && choice.colorSpace == kVkColorSpaceSrgbNonlinear);
        assert(choice.toneMapToSdr && choice.dataspaceOverride == 0);
    }
    // Only HDR spaces, no SRGB_NONLINEAR to tone map onto: the first listed format as it is
    const OutputSurfaceFormat odd[] = {{kR16G16B16A16Sfloat, kExtendedSrgbLinear}};
    OutputChoice choice = choose(odd, true, kHdrTransferPq);
    assert(choice.toneMapToSdr && choice.format == kR16G16B16A16Sfloat && choice.colorSpace == kExtendedSrgbLinear);
}

static void sdrIsUnchanged() {
    for (const OutputChoice& choice : {choose(kAndroid14, false, kHdrTransferPq), choose(kAndroid15, false, kHdrTransferHlg)}) {
        assert(choice.format == kVkFormatA2B10G10R10Unorm && choice.colorSpace == kVkColorSpaceSrgbNonlinear);
        assert(choice.dataspaceOverride == 0 && !choice.toneMapToSdr);
    }
    OutputChoice eight = choose(kAndroid15, false, kHdrTransferPq, false);
    assert(eight.format == kVkFormatR8G8B8A8Unorm && eight.colorSpace == kVkColorSpaceSrgbNonlinear);
    assert(choose(kEightBit, false, kHdrTransferPq).format == kVkFormatR8G8B8A8Unorm);
    assert(vkSelectOutput(nullptr, 0, true, kHdrTransferPq, true).format == 0);
}

static void hlgCurve() {
    same(glsl::hlgInverseOetf(0.0f), 0.0, 1e-7);
    same(glsl::hlgInverseOetf(-0.1f), 0.0, 1e-7);
    // The two pieces meet at the knee, and the full signal is full scene light
    same(glsl::hlgInverseOetf(0.5f), 1.0 / 12.0, 1e-6);
    same(glsl::hlgInverseOetf(0.5001f), 1.0 / 12.0, 1e-4);
    same(glsl::hlgInverseOetf(1.0f), 1.0, 1e-4);
    // BT.2100 Table 5: the signal for 75% HLG reference white
    same(glsl::hlgInverseOetf(0.75f), 0.26496, 1e-4);
    assert(glsl::HLG_SYSTEM_GAMMA == 1.2f);
}

int main() {
    android14GoesThroughTheWindowDataspace();
    android15UsesTheNativeColorSpace();
    withoutTenBitHdrIsToneMapped();
    sdrIsUnchanged();
    hlgCurve();
    return 0;
}

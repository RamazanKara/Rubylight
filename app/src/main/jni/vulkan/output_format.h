#pragma once

// The swapchain format and colour space the renderer presents in, and how HDR reaches the
// compositor. Values match VkFormat, VkColorSpaceKHR and ADataSpace, kept independent of the
// Vulkan and NDK headers so the host tests can run it (vulkan_renderer.cpp asserts them).

#include <cstddef>
#include <cstdint>

namespace vkr {

constexpr int32_t kVkFormatR8G8B8A8Unorm = 37;
constexpr int32_t kVkFormatA2B10G10R10Unorm = 64;
constexpr int32_t kVkColorSpaceSrgbNonlinear = 0;
constexpr int32_t kVkColorSpaceHdr10St2084 = 1000104008;
constexpr int32_t kVkColorSpaceHdr10Hlg = 1000104010;
constexpr int32_t kDataspaceBt2020Pq = 163971072;
constexpr int32_t kDataspaceBt2020Hlg = 168165376;

// RendererConfig::hdrTransfer: the transfer of the stream's HDR
constexpr int kHdrTransferPq = 0;
constexpr int kHdrTransferHlg = 1;

struct OutputSurfaceFormat {
    int32_t format, colorSpace;
};

struct OutputChoice {
    int32_t format = 0;
    int32_t colorSpace = kVkColorSpaceSrgbNonlinear;
    // The ADataSpace to set on the window after every swapchain (re)creation, 0 for none: the
    // swapchain's own colour space says SRGB, and the window says the picture is HDR
    int32_t dataspaceOverride = 0;
    // HDR the surface cannot take: the shader maps it to SDR
    bool toneMapToSdr = false;
};

inline int32_t outputHdrColorSpace(int transfer) {
    return transfer == kHdrTransferHlg ? kVkColorSpaceHdr10Hlg : kVkColorSpaceHdr10St2084;
}

inline int32_t outputHdrDataspace(int transfer) {
    return transfer == kHdrTransferHlg ? kDataspaceBt2020Hlg : kDataspaceBt2020Pq;
}

// HDR is tried in this order, and the picture passes through unchanged in the first two:
// - a ten-bit format in the native HDR10_ST2084 or HDR10_HLG colour space, which the Android
//   loader lists from release 15 (with VK_EXT_swapchain_colorspace);
// - a ten-bit SRGB_NONLINEAR format with the window dataspace set to BT2020_PQ or BT2020_HLG, as
//   the loaders of releases 13 and 14 list no HDR colour space at all;
// - tone mapping to SDR on the best SDR format.
// Without HDR, a ten-bit stream gets a ten-bit SDR format where there is one. When nothing fits,
// the first listed format is used as it is (format stays 0 for an empty list).
inline OutputChoice vkSelectOutput(const OutputSurfaceFormat* formats, size_t count, bool hdr, int transfer,
                                   bool tenBit) {
    auto has = [&](int32_t format, int32_t colorSpace) {
        for (size_t i = 0; i < count; ++i) {
            if (formats[i].format == format && formats[i].colorSpace == colorSpace) return true;
        }
        return false;
    };
    OutputChoice choice;
    if (hdr) {
        const int32_t colorSpace = outputHdrColorSpace(transfer);
        if (has(kVkFormatA2B10G10R10Unorm, colorSpace)) {
            choice.format = kVkFormatA2B10G10R10Unorm;
            choice.colorSpace = colorSpace;
            return choice;
        }
        if (has(kVkFormatA2B10G10R10Unorm, kVkColorSpaceSrgbNonlinear)) {
            choice.format = kVkFormatA2B10G10R10Unorm;
            choice.dataspaceOverride = outputHdrDataspace(transfer);
            return choice;
        }
        choice.toneMapToSdr = true;
    }
    if (tenBit && has(kVkFormatA2B10G10R10Unorm, kVkColorSpaceSrgbNonlinear)) {
        choice.format = kVkFormatA2B10G10R10Unorm;
    }
    else if (has(kVkFormatR8G8B8A8Unorm, kVkColorSpaceSrgbNonlinear)) {
        choice.format = kVkFormatR8G8B8A8Unorm;
    }
    else if (count > 0) {
        choice.format = formats[0].format;
        choice.colorSpace = formats[0].colorSpace;
    }
    return choice;
}

}  // namespace vkr

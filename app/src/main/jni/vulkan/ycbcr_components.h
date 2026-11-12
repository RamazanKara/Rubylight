#pragma once

// The component swizzle of the decoder buffers' YCbCr conversion. Values match VkComponentSwizzle
// and VkFormat (vulkan_renderer.cpp asserts them) and the PCI vendor ID in
// VkPhysicalDeviceProperties::vendorID, kept independent of the Vulkan headers so the host tests
// can run it.

#include <cstdint>

namespace vkr {

constexpr int32_t kVkComponentSwizzleIdentity = 0;
constexpr int32_t kVkComponentSwizzleR = 3;
constexpr int32_t kVkComponentSwizzleB = 5;
constexpr int32_t kVkFormatG10X6B10X6R10X6TwoPlane420Unorm3Pack16 = 1000156021;
constexpr uint32_t kVkVendorNvidia = 0x10DE;

// NVIDIA's Android driver (Tegra X1: the Shield TV) reports the decoder's 10-bit P010 buffers as
// VK_FORMAT_G10X6_B10X6R10X6_2PLANE_420_UNORM_3PACK16 with components that read the chroma
// backwards: Cr comes out where the conversion reads Cb and the other way round, so red shows blue
// and yellow-green cyan. Its 8-bit NV12 buffers sample correctly. On Android, NVIDIA means Tegra.
// Follows MoreOrLessSoftware/moonlight-android b44610fb4.
inline bool swapsChroma(uint32_t vendorId, int32_t format) {
    return vendorId == kVkVendorNvidia && format == kVkFormatG10X6B10X6R10X6TwoPlane420Unorm3Pack16;
}

// The components for VkSamplerYcbcrConversionCreateInfo: the buffer's
// samplerYcbcrConversionComponents, with R and B swapped where swapsChroma says the driver has the
// chroma backwards. The model conversion reads Cr through R and Cb through B, so swapping what R
// and B read swaps Cb and Cr. Identity is spelled out before the swap, since it names a different
// channel in each slot, and a slot left reading its own channel goes back to identity. Mapping is
// VkComponentMapping, or any struct with r, g, b and a members.
template <typename Mapping>
Mapping ycbcrComponents(Mapping reported, uint32_t vendorId, int32_t format) {
    if (!swapsChroma(vendorId, format)) {
        return reported;
    }
    using Swizzle = decltype(reported.r);
    const int32_t reportedR = static_cast<int32_t>(reported.r);
    const int32_t reportedB = static_cast<int32_t>(reported.b);
    const int32_t r = reportedR == kVkComponentSwizzleIdentity ? kVkComponentSwizzleR : reportedR;
    const int32_t b = reportedB == kVkComponentSwizzleIdentity ? kVkComponentSwizzleB : reportedB;
    reported.r = static_cast<Swizzle>(b == kVkComponentSwizzleR ? kVkComponentSwizzleIdentity : b);
    reported.b = static_cast<Swizzle>(r == kVkComponentSwizzleB ? kVkComponentSwizzleIdentity : r);
    return reported;
}

}  // namespace vkr

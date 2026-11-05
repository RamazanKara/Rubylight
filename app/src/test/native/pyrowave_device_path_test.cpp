#include <algorithm>
#include <cassert>
#include <cstring>
#include "../../main/jni/pyrowave-renderer/device_path.h"

static const uint32_t V1_0 = 1u << 22;
static const uint32_t V1_4 = (1u << 22) | (4u << 12);

static bool has(const std::vector<const char *> &names, const char *name) {
    return std::any_of(names.begin(), names.end(), [name](const char *n) { return !strcmp(n, name); });
}

static void vulkan13DevicesKeepTheirPath() {
    const uint32_t instance = pyroInstanceApiVersion(V1_4, false);
    assert(instance == PYRO_VULKAN_1_3);
    for (uint32_t device : {PYRO_VULKAN_1_3, V1_4}) {
        const uint32_t core = pyroCoreVersion(device, instance);
        assert(!pyroLegacySync(core));
        assert(pyroQueueCount(2, false) == 2);
        assert(pyroPathExtensions(core, true, true, true).empty());
        // Still refused without what they always needed.
        assert(!pyroSyncUsable(core, false, true));
        assert(!pyroSyncUsable(core, true, false));
        assert(pyroSyncUsable(core, true, true));
        assert(!pyroPayloadReadable(core, false, PYRO_TEXEL_BUFFER_ELEMENTS));
        assert(pyroPayloadReadable(core, true, 0));
        assert(!pyroNeedsRenderPass2(core));
    }
}

static void theLowerOfLoaderAndDeviceDecides() {
    // A Vulkan 1.1 or 1.2 loader asks for 1.1, so a newer GPU behind it uses the extensions.
    for (uint32_t loader : {PYRO_VULKAN_1_1, PYRO_VULKAN_1_2}) {
        assert(pyroInstanceApiVersion(loader, false) == PYRO_VULKAN_1_1);
        assert(pyroLegacySync(pyroCoreVersion(PYRO_VULKAN_1_3, pyroInstanceApiVersion(loader, false))));
    }
    // A Vulkan 1.2 GPU behind a 1.3 loader stays on 1.2's core structs.
    const uint32_t core = pyroCoreVersion(PYRO_VULKAN_1_2, PYRO_VULKAN_1_3);
    assert(core == PYRO_VULKAN_1_2 && pyroLegacySync(core) && !pyroNeedsRenderPass2(core));
    assert(pyroCoreVersion(V1_0, PYRO_VULKAN_1_3) < PYRO_VULKAN_1_1);
}

static void theTestSwitchTakesTheVulkan11Path() {
    const uint32_t core = pyroCoreVersion(PYRO_VULKAN_1_3, pyroInstanceApiVersion(V1_4, true));
    assert(core == PYRO_VULKAN_1_1 && pyroLegacySync(core) && pyroNeedsRenderPass2(core));
    assert(pyroQueueCount(2, true) == 1);
}

static void olderDevicesShareOneQueueAndMayLackStorage8() {
    assert(pyroQueueCount(2, true) == 1);
    assert(pyroQueueCount(1, false) == 1);
    assert(pyroSyncUsable(PYRO_VULKAN_1_1, false, false));
    assert(pyroSyncUsable(PYRO_VULKAN_1_2, false, false));
    assert(pyroPayloadReadable(PYRO_VULKAN_1_1, false, PYRO_TEXEL_BUFFER_ELEMENTS));
    assert(!pyroPayloadReadable(PYRO_VULKAN_1_1, false, PYRO_TEXEL_BUFFER_ELEMENTS - 1));
    assert(pyroPayloadReadable(PYRO_VULKAN_1_2, true, 0));
}

static void pathExtensions() {
    const auto v12 = pyroPathExtensions(PYRO_VULKAN_1_2, true, true, true);
    assert(v12.size() == 1 && has(v12, "VK_EXT_subgroup_size_control"));

    const auto v11 = pyroPathExtensions(PYRO_VULKAN_1_1, true, true, true);
    assert(has(v11, "VK_EXT_subgroup_size_control") && has(v11, "VK_KHR_create_renderpass2"));
    assert(has(v11, "VK_KHR_8bit_storage") && has(v11, "VK_KHR_shader_float16_int8"));
    // Granite picks Adreno and Mali's fragment path from the driver id this extension gives it.
    assert(has(v11, "VK_KHR_driver_properties"));

    const auto bare = pyroPathExtensions(PYRO_VULKAN_1_1, false, false, false);
    assert(bare.size() == 2);
    assert(!has(bare, "VK_KHR_8bit_storage") && !has(bare, "VK_KHR_driver_properties"));
}

static void adrenoAndMaliNeedTheirDriverIdBelowVulkan12() {
    for (uint32_t vendor : {PYRO_VENDOR_QUALCOMM, PYRO_VENDOR_ARM}) {
        assert(!pyroDriverIdKnown(PYRO_VULKAN_1_1, vendor, false));
        assert(pyroDriverIdKnown(PYRO_VULKAN_1_1, vendor, true));
        // Core from Vulkan 1.2 on.
        assert(pyroDriverIdKnown(PYRO_VULKAN_1_2, vendor, false));
        assert(pyroDriverIdKnown(PYRO_VULKAN_1_3, vendor, false));
    }
    // The Shield's NVIDIA and others take no path by driver id.
    assert(pyroDriverIdKnown(PYRO_VULKAN_1_1, 0x10DE, false));
    assert(pyroDriverIdKnown(PYRO_VULKAN_1_1, 0x1010, false));
}

int main() {
    adrenoAndMaliNeedTheirDriverIdBelowVulkan12();
    vulkan13DevicesKeepTheirPath();
    theLowerOfLoaderAndDeviceDecides();
    theTestSwitchTakesTheVulkan11Path();
    olderDevicesShareOneQueueAndMayLackStorage8();
    pathExtensions();
}

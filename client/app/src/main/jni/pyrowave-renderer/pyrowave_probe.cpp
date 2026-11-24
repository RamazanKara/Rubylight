// SPDX-License-Identifier: GPL-3.0-only
// Adapted from joemossjr16/artemis-android-pyrowave 387d3a5c.
// Vulkan capability/readiness probing, custom Adreno driver loading and KGSL clock control.

#include "pyrowave_vulkan.h"

namespace pyrowave_renderer {

std::atomic<int> hdrSurfaceFailure{0};
std::mutex readinessMutex;

namespace {
// Adreno power control from the kernel UAPI header msm_kgsl.h, as used by emulators to pin
// GPU clocks: property value 0 turns power management off, 1 restores it.
struct KgslProperty {
    unsigned int type;
    void *value;
    size_t sizebytes;
};
constexpr unsigned int KGSL_PROP_PWRCTRL = 0x0E;
constexpr unsigned long KGSL_IOCTL_SETPROPERTY = _IOW(0x09, 0x32, KgslProperty);
}  // namespace

bool setAdrenoMaxClocks(bool enabled) {
    int fd = open("/dev/kgsl-3d0", O_RDWR | O_CLOEXEC);
    if (fd < 0) return false;
    unsigned int powerControl = enabled ? 0u : 1u;
    KgslProperty property = {KGSL_PROP_PWRCTRL, &powerControl, sizeof(powerControl)};
    bool ok = ioctl(fd, KGSL_IOCTL_SETPROPERTY, &property) == 0;
    close(fd);
    LOGI("Adreno max clocks %s: %s", enabled ? "on" : "off", ok ? "applied" : "rejected");
    return ok;
}

CustomDriverLibrary *customDriverLibrary(const std::string &hooks, const std::string &directory,
                                         const std::string &name) {
#if defined(__aarch64__)
    static std::mutex mutex;
    // Hook namespaces retain their parameters. Keep each immutable driver's loader alive
    // until process exit rather than unloading code another Vulkan instance may still use.
    static std::map<std::string, std::unique_ptr<CustomDriverLibrary>> libraries;
    std::lock_guard<std::mutex> lock(mutex);
    auto &driver = libraries[directory];
    if (!driver) {
        driver = std::make_unique<CustomDriverLibrary>();
        // The opt-in library's namespace resolver must never run for system-driver users.
        driver->adrenotools = dlopen("libadrenotools.so", RTLD_NOW | RTLD_LOCAL);
        auto openDriver = driver->adrenotools == nullptr ? nullptr :
            reinterpret_cast<decltype(&adrenotools_open_libvulkan)>(dlsym(driver->adrenotools, "adrenotools_open_libvulkan"));
        if (driver->adrenotools == nullptr) {
            const char *error = dlerror();
            snprintf(driver->error, sizeof(driver->error), "libadrenotools.so: %s", error ? error : "not found");
        } else if (openDriver == nullptr) {
            snprintf(driver->error, sizeof(driver->error), "libadrenotools.so has no adrenotools_open_libvulkan");
        } else {
            driver->library = openDriver(RTLD_NOW | RTLD_LOCAL, ADRENOTOOLS_DRIVER_CUSTOM,
                    nullptr, hooks.c_str(), directory.c_str(), name.c_str(), nullptr, nullptr, &driver->loaded,
                    driver->error, sizeof(driver->error));
            if (driver->library == nullptr && driver->error[0] == '\0') {
                snprintf(driver->error, sizeof(driver->error),
                         "adrenotools could not open the system Vulkan loader with its hooks");
            }
        }
    }
    return driver.get();
#else
    (void) hooks;
    (void) directory;
    (void) name;
    return nullptr;
#endif
}

std::string jniString(JNIEnv *env, jstring value) {
    if (value == nullptr) return {};
    const char *characters = env->GetStringUTFChars(value, nullptr);
    if (characters == nullptr) return {};
    std::string result(characters);
    env->ReleaseStringUTFChars(value, characters);
    return result;
}

bool hasExtension(const std::vector<VkExtensionProperties> &extensions, const char *name) {
    return std::any_of(extensions.begin(), extensions.end(),
                       [name](const VkExtensionProperties &ext) { return !strcmp(ext.extensionName, name); });
}

// instanceVersion is the instance's apiVersion: core features above it are not usable on the device.
FeatureProbe probeFeatures(const VulkanLoader &vk, VkPhysicalDevice device, bool tenBit, uint32_t instanceVersion) {
    FeatureProbe probe;
    VkPhysicalDeviceProperties props;
    vk.GetPhysicalDeviceProperties(device, &props);
    probe.apiVersion = props.apiVersion;
    std::memcpy(probe.name, props.deviceName, sizeof(probe.name));
    const uint32_t coreVersion = pyroCoreVersion(props.apiVersion, instanceVersion);
    if (coreVersion < VK_API_VERSION_1_1) {
        return probe;
    }
    const bool core12 = coreVersion >= VK_API_VERSION_1_2;
    const bool core13 = coreVersion >= VK_API_VERSION_1_3;
    probe.legacySync = pyroLegacySync(coreVersion);

    uint32_t extensionCount = 0;
    probe.reason = PROBE_FAILED;
    if (vk.EnumerateDeviceExtensionProperties(device, nullptr, &extensionCount, nullptr) != VK_SUCCESS) return probe;
    std::vector<VkExtensionProperties> extensions(extensionCount);
    if (vk.EnumerateDeviceExtensionProperties(device, nullptr, &extensionCount, extensions.data()) != VK_SUCCESS) return probe;
    extensions.resize(extensionCount);
    if (pyroNeedsSubgroupSizeControlExtension(coreVersion) && !hasExtension(extensions, VK_EXT_SUBGROUP_SIZE_CONTROL_EXTENSION_NAME)) {
        probe.reason = MISSING_SUBGROUP_OPS;
        return probe;
    }
    if (pyroNeedsRenderPass2(coreVersion) && !hasExtension(extensions, VK_KHR_CREATE_RENDERPASS_2_EXTENSION_NAME)) {
        probe.reason = MISSING_DEVICE_FEATURES;
        return probe;
    }
    if (!pyroDriverIdKnown(coreVersion, props.vendorID, hasExtension(extensions, VK_KHR_DRIVER_PROPERTIES_EXTENSION_NAME))) {
        LOGW("PyroWave refused: %s has no VK_KHR_driver_properties, so its decode path can't be chosen", props.deviceName);
        probe.reason = MISSING_DEVICE_FEATURES;
        return probe;
    }
    const bool storage8Extension = !core12 && hasExtension(extensions, VK_KHR_8BIT_STORAGE_EXTENSION_NAME);
    probe.float16Extension = !core12 && hasExtension(extensions, VK_KHR_SHADER_FLOAT16_INT8_EXTENSION_NAME);

    // The core feature structs only where the device has that core version, else their extensions.
    VkPhysicalDeviceVulkan13Features f13 = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_VULKAN_1_3_FEATURES};
    VkPhysicalDeviceVulkan12Features f12 = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_VULKAN_1_2_FEATURES};
    VkPhysicalDeviceVulkan11Features f11 = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_VULKAN_1_1_FEATURES};
    VkPhysicalDeviceSubgroupSizeControlFeaturesEXT sizeControl = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_SUBGROUP_SIZE_CONTROL_FEATURES_EXT};
    VkPhysicalDevice16BitStorageFeatures storage16 = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_16BIT_STORAGE_FEATURES};
    VkPhysicalDevice8BitStorageFeaturesKHR storage8 = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_8BIT_STORAGE_FEATURES_KHR};
    VkPhysicalDeviceShaderFloat16Int8FeaturesKHR float16Int8 = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_SHADER_FLOAT16_INT8_FEATURES_KHR};
    VkPhysicalDeviceFeatures2 f2 = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_FEATURES_2};
    void **next = &f2.pNext;
    const auto chain = [&next](auto &feature) {
        *next = &feature;
        next = &feature.pNext;
    };
    if (core12) {
        chain(f11);
        chain(f12);
    } else {
        chain(storage16);
        if (storage8Extension) chain(storage8);
        if (probe.float16Extension) chain(float16Int8);
    }
    if (core13) chain(f13); else chain(sizeControl);
    vk.GetPhysicalDeviceFeatures2(device, &f2);

    VkPhysicalDeviceSubgroupProperties subgroup = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_SUBGROUP_PROPERTIES};
    VkPhysicalDeviceVulkan13Properties p13 = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_VULKAN_1_3_PROPERTIES};
    VkPhysicalDeviceSubgroupSizeControlPropertiesEXT sizeControlProps = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_SUBGROUP_SIZE_CONTROL_PROPERTIES_EXT};
    if (core13) subgroup.pNext = &p13; else subgroup.pNext = &sizeControlProps;
    VkPhysicalDeviceProperties2 p2 = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PROPERTIES_2};
    p2.pNext = &subgroup;
    vk.GetPhysicalDeviceProperties2(device, &p2);
    const uint32_t minSubgroupSize = core13 ? p13.minSubgroupSize : sizeControlProps.minSubgroupSize;
    const uint32_t maxSubgroupSize = core13 ? p13.maxSubgroupSize : sizeControlProps.maxSubgroupSize;
    const VkShaderStageFlags requiredSubgroupSizeStages =
        core13 ? p13.requiredSubgroupSizeStages : sizeControlProps.requiredSubgroupSizeStages;
    constexpr VkSubgroupFeatureFlags required = VK_SUBGROUP_FEATURE_BASIC_BIT | VK_SUBGROUP_FEATURE_VOTE_BIT |
        VK_SUBGROUP_FEATURE_BALLOT_BIT | VK_SUBGROUP_FEATURE_ARITHMETIC_BIT |
        VK_SUBGROUP_FEATURE_SHUFFLE_BIT | VK_SUBGROUP_FEATURE_SHUFFLE_RELATIVE_BIT;
    bool subgroups = (subgroup.supportedOperations & required) == required &&
        (subgroup.supportedStages & VK_SHADER_STAGE_COMPUTE_BIT) &&
        ((minSubgroupSize >= 4 && maxSubgroupSize <= 128) ||
         ((requiredSubgroupSizeStages & VK_SHADER_STAGE_COMPUTE_BIT) &&
          minSubgroupSize <= 128 && maxSubgroupSize >= 4));
    const bool sizeControlFeatures = core13 ? f13.subgroupSizeControl && f13.computeFullSubgroups :
        sizeControl.subgroupSizeControl && sizeControl.computeFullSubgroups;
    if (!subgroups || !sizeControlFeatures) {
        probe.reason = MISSING_SUBGROUP_OPS;
        return probe;
    }
    const bool storage16Feature = core12 ? f11.storageBuffer16BitAccess : storage16.storageBuffer16BitAccess;
    probe.storage8 = core12 ? f12.storageBuffer8BitAccess : storage8Extension && storage8.storageBuffer8BitAccess;
    // adb shell setprop debug.nightjar.pyro_no_storage8 1 runs the texel buffer path of devices
    // without 8-bit storage on one we have (with pyro_vk11, below Vulkan 1.3).
    char noStorage8[PROP_VALUE_MAX] = {};
    __system_property_get("debug.nightjar.pyro_no_storage8", noStorage8);
    if (std::strcmp(noStorage8, "1") == 0) probe.storage8 = false;
    const bool payloadAccess = pyroPayloadReadable(probe.storage8, props.limits.maxTexelBufferElements);
    // Vulkan 1.3 devices keep their timeline/synchronization2 path; older ones use legacySync.
    const bool sync = pyroSyncUsable(coreVersion, f12.timelineSemaphore, f13.synchronization2);
    if (!f2.features.shaderStorageImageWriteWithoutFormat || !f2.features.shaderInt16 ||
        !storage16Feature || !payloadAccess || !sync) {
        probe.reason = MISSING_DEVICE_FEATURES;
        return probe;
    }
    probe.float16 = core12 ? f12.shaderFloat16 : probe.float16Extension && float16Int8.shaderFloat16;
    if (props.limits.maxImageArrayLayers < 12 || props.limits.maxComputeWorkGroupInvocations < 128 ||
        props.limits.maxComputeWorkGroupSize[0] < 128) {
        probe.reason = INSUFFICIENT_DEVICE_LIMITS;
        return probe;
    }
    // Mixed-precision wavelets use FP16 high bands and FP32 low bands.
    for (auto format : {VK_FORMAT_R16_SFLOAT, VK_FORMAT_R32_SFLOAT, VK_FORMAT_R16G16_SFLOAT}) {
        VkFormatProperties formats;
        vk.GetPhysicalDeviceFormatProperties(device, format, &formats);
        VkFormatFeatureFlags required = VK_FORMAT_FEATURE_SAMPLED_IMAGE_BIT |
            VK_FORMAT_FEATURE_STORAGE_IMAGE_BIT | VK_FORMAT_FEATURE_COLOR_ATTACHMENT_BIT;
        if ((formats.optimalTilingFeatures & required) != required) {
            probe.reason = MISSING_IMAGE_FORMATS;
            return probe;
        }
    }
    VkFormatProperties plane;
    vk.GetPhysicalDeviceFormatProperties(device, tenBit ? VK_FORMAT_R16_UNORM : VK_FORMAT_R8_UNORM, &plane);
    VkFormatFeatureFlags sampled = VK_FORMAT_FEATURE_SAMPLED_IMAGE_BIT | VK_FORMAT_FEATURE_SAMPLED_IMAGE_FILTER_LINEAR_BIT;
    if ((plane.optimalTilingFeatures & sampled) != sampled ||
        !(plane.optimalTilingFeatures & (VK_FORMAT_FEATURE_STORAGE_IMAGE_BIT | VK_FORMAT_FEATURE_COLOR_ATTACHMENT_BIT))) {
        probe.reason = MISSING_IMAGE_FORMATS;
        return probe;
    }
    uint32_t familyCount = 0;
    vk.GetPhysicalDeviceQueueFamilyProperties(device, &familyCount, nullptr);
    std::vector<VkQueueFamilyProperties> families(familyCount);
    vk.GetPhysicalDeviceQueueFamilyProperties(device, &familyCount, families.data());
    bool graphicsCompute = false;
    for (uint32_t i = 0; i < familyCount; ++i) {
        graphicsCompute |= (families[i].queueFlags & (VK_QUEUE_GRAPHICS_BIT | VK_QUEUE_COMPUTE_BIT)) ==
            (VK_QUEUE_GRAPHICS_BIT | VK_QUEUE_COMPUTE_BIT);
    }
    if (!graphicsCompute) {
        probe.reason = MISSING_GRAPHICS_QUEUE;
        return probe;
    }
    const bool swapchain = hasExtension(extensions, VK_KHR_SWAPCHAIN_EXTENSION_NAME);
    const bool metadata = hasExtension(extensions, VK_EXT_HDR_METADATA_EXTENSION_NAME);
    probe.reason = !swapchain ? MISSING_SWAPCHAIN : (tenBit && !metadata ? MISSING_HDR : READY);
    return probe;
}

// Vulkan 1.3 where the loader has it; Granite takes the device's core version as the lower of
// this and the device's own, so a 1.1 loader keeps a 1.3 device on its extensions.
// adb shell setprop debug.nightjar.pyro_vk11 1 does the same on purpose, to run the path of
// older devices on one we have.
uint32_t instanceApiVersion(uint32_t loaderVersion) {
    char value[PROP_VALUE_MAX] = {};
    __system_property_get("debug.nightjar.pyro_vk11", value);
    const bool forced = std::strcmp(value, "1") == 0;
    if (forced) LOGW("PyroWave forced below Vulkan 1.3 by debug.nightjar.pyro_vk11");
    return pyroInstanceApiVersion(loaderVersion, forced);
}

bool apiVersionSupported() {
    uint32_t major = 0, minor = 0, patch = 0;
    pyrowave_get_api_version(&major, &minor, &patch);
    if (major != PYROWAVE_API_VERSION_MAJOR || minor != PYROWAVE_API_VERSION_MINOR) {
        LOGE("PyroWave runtime %u.%u.%u does not match %u.%u", major, minor, patch,
             PYROWAVE_API_VERSION_MAJOR, PYROWAVE_API_VERSION_MINOR);
        return false;
    }
    return true;
}

// stage says how far the probe got (device_path.h), which names a custom driver's failure.
Readiness probeVulkan(bool tenBit, VulkanLoader &vk, PyroProbeStage &stage) {
    stage = PYRO_STAGE_LOADER;
    if (!vk.loadGlobal()) {
        return NEEDS_VULKAN;
    }
    uint32_t loaderVersion = 0;
    if (vk.EnumerateInstanceVersion(&loaderVersion) != VK_SUCCESS || loaderVersion < VK_API_VERSION_1_1) {
        LOGI("Vulkan loader is older than 1.1; PyroWave unavailable");
        return NEEDS_VULKAN;
    }

    uint32_t extensionCount = 0;
    if (vk.EnumerateInstanceExtensionProperties(nullptr, &extensionCount, nullptr) != VK_SUCCESS) return PROBE_FAILED;
    std::vector<VkExtensionProperties> extensions(extensionCount);
    if (vk.EnumerateInstanceExtensionProperties(nullptr, &extensionCount, extensions.data()) != VK_SUCCESS) return PROBE_FAILED;
    extensions.resize(extensionCount);
    for (const char *required : INSTANCE_EXTENSIONS) {
        if (std::none_of(extensions.begin(), extensions.end(), [required](const VkExtensionProperties &ext) {
            return !strcmp(ext.extensionName, required);
        })) {
            stage = PYRO_STAGE_CHECKED;
            return MISSING_SWAPCHAIN;
        }
    }
    if (tenBit && std::none_of(extensions.begin(), extensions.end(), [](const VkExtensionProperties &ext) {
        return !strcmp(ext.extensionName, VK_EXT_SWAPCHAIN_COLOR_SPACE_EXTENSION_NAME);
    })) {
        stage = PYRO_STAGE_CHECKED;
        return MISSING_HDR;
    }

    VkApplicationInfo appInfo = {VK_STRUCTURE_TYPE_APPLICATION_INFO};
    appInfo.apiVersion = instanceApiVersion(loaderVersion);
    VkInstanceCreateInfo instanceInfo = {VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO};
    instanceInfo.pApplicationInfo = &appInfo;
    VkInstance instance = VK_NULL_HANDLE;
    if (vk.CreateInstance(&instanceInfo, nullptr, &instance) != VK_SUCCESS) {
        return PROBE_FAILED;
    }
    // Only the functions the probe needs; surface functions may be absent here.
    vk.EnumeratePhysicalDevices = reinterpret_cast<PFN_vkEnumeratePhysicalDevices>(
        vk.GetInstanceProcAddr(instance, "vkEnumeratePhysicalDevices"));
    vk.GetPhysicalDeviceProperties = reinterpret_cast<PFN_vkGetPhysicalDeviceProperties>(
        vk.GetInstanceProcAddr(instance, "vkGetPhysicalDeviceProperties"));
    vk.GetPhysicalDeviceProperties2 = reinterpret_cast<PFN_vkGetPhysicalDeviceProperties2>(
        vk.GetInstanceProcAddr(instance, "vkGetPhysicalDeviceProperties2"));
    vk.GetPhysicalDeviceFeatures2 = reinterpret_cast<PFN_vkGetPhysicalDeviceFeatures2>(
        vk.GetInstanceProcAddr(instance, "vkGetPhysicalDeviceFeatures2"));
    vk.GetPhysicalDeviceFormatProperties = reinterpret_cast<PFN_vkGetPhysicalDeviceFormatProperties>(
        vk.GetInstanceProcAddr(instance, "vkGetPhysicalDeviceFormatProperties"));
    vk.GetPhysicalDeviceQueueFamilyProperties = reinterpret_cast<PFN_vkGetPhysicalDeviceQueueFamilyProperties>(
        vk.GetInstanceProcAddr(instance, "vkGetPhysicalDeviceQueueFamilyProperties"));
    vk.EnumerateDeviceExtensionProperties = reinterpret_cast<PFN_vkEnumerateDeviceExtensionProperties>(
        vk.GetInstanceProcAddr(instance, "vkEnumerateDeviceExtensionProperties"));
    vk.DestroyInstance = reinterpret_cast<PFN_vkDestroyInstance>(
        vk.GetInstanceProcAddr(instance, "vkDestroyInstance"));

    if (!vk.EnumeratePhysicalDevices || !vk.GetPhysicalDeviceProperties ||
        !vk.GetPhysicalDeviceProperties2 || !vk.GetPhysicalDeviceFeatures2 || !vk.DestroyInstance ||
        !vk.GetPhysicalDeviceFormatProperties || !vk.GetPhysicalDeviceQueueFamilyProperties ||
        !vk.EnumerateDeviceExtensionProperties) {
        if (vk.DestroyInstance) vk.DestroyInstance(instance, nullptr);
        return PROBE_FAILED;
    }

    stage = PYRO_STAGE_DEVICES;
    Readiness readiness = PROBE_FAILED;
    uint32_t count = 0;
    if (vk.EnumeratePhysicalDevices(instance, &count, nullptr) == VK_SUCCESS && count > 0) {
        std::vector<VkPhysicalDevice> devices(count);
        if (vk.EnumeratePhysicalDevices(instance, &count, devices.data()) == VK_SUCCESS && count > 0) {
            devices.resize(count);
            stage = PYRO_STAGE_CHECKED;
            readiness = NEEDS_VULKAN;
            for (auto device : devices) {
                auto probe = probeFeatures(vk, device, tenBit, appInfo.apiVersion);
                LOGI("%s (Vulkan %u.%u) PyroWave readiness: %d", probe.name, VK_API_VERSION_MAJOR(probe.apiVersion),
                     VK_API_VERSION_MINOR(probe.apiVersion), probe.reason);
                if (probe.reason == READY) {
                    readiness = READY;
                    break;
                }
                readiness = std::max(readiness, probe.reason);
            }
        }
    }
    vk.DestroyInstance(instance, nullptr);
    return readiness;
}

Readiness probeReadiness(bool tenBit, CustomDriverLibrary *custom) {
    if (!apiVersionSupported()) {
        return PROBE_FAILED;
    }
    VulkanLoader vk(custom);
    PyroProbeStage stage = PYRO_STAGE_LOADER;
    const Readiness readiness = probeVulkan(tenBit, vk, stage);
    if (custom == nullptr) return readiness;
    // The hook falls back to the system driver when the custom one does not load: a probe of that
    // must not pass for the custom driver, and a stop before any device check names its stage.
    const auto result = static_cast<Readiness>(pyroCustomDriverReadiness(
        custom->library != nullptr, vk.usingRequestedDriver(), vk.customNotVulkan(), stage, readiness));
    if (result != READY) {
        LOGW("Custom Vulkan driver PyroWave readiness: %d (probe %d, stage %d)%s%s", result, readiness, stage,
             custom->error[0] ? ": " : "", custom->error);
    }
    return result;
}

}  // namespace pyrowave_renderer

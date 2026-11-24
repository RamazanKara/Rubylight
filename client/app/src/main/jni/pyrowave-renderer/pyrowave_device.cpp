// SPDX-License-Identifier: GPL-3.0-only
// Adapted from joemossjr16/artemis-android-pyrowave 387d3a5c.
// PyroWave Vulkan instance/device creation, decoder setup, Y/Cb/Cr planes and teardown.

#include "pyrowave_renderer_internal.h"

namespace pyrowave_renderer {

bool Renderer::create(ANativeWindow *nativeWindow, int streamWidth, int streamHeight, int frameRate, float displayRefreshRate,
            float topRefreshRate, int pacingMode,
            bool fullChroma, bool tenBitOutput, bool fullRangeOutput, bool stretchOutput, int upscale, float sharpness,
            bool preacquireSetting) {
    window = nativeWindow;
    upscaleMode = upscale;
    stretch = stretchOutput;
    edgeSharpness = sharpness;
    chroma444 = fullChroma;
    tenBit = hdr = tenBitOutput;
    if (tenBitOutput) hdrSurfaceFailure = 0;
    fullRange = fullRangeOutput;
    frameRateHz = frameRate > 0 ? frameRate : 60;
    displayRefreshRateHz = displayRefreshRate;
    framePacingMode = pacingMode >= 0 && pacingMode <= HOST_TIMES_MODE ? pacingMode : 0;
    if (framePacingMode == HOST_TIMES_MODE && !createPresentClock(topRefreshRate)) framePacingMode = 0;
    const int64_t periodNs = displayRefreshRate > 1.0f && displayRefreshRate <= 1000.0f ?
            int64_t(1'000'000'000.0 / displayRefreshRate) : 16'666'667;
    presentTiming.configure(static_cast<PyroWavePresentTiming::Mode>(framePacingMode), periodNs);
    presentTiming.setStreamInterval(1'000'000'000LL / frameRateHz);
    width = uint32_t(streamWidth);
    height = uint32_t(streamHeight);
    records = PyroWaveRecords(width, height, chroma444);
    submissionDiagnostics.configure();
    presentationDiagnostics.configure(submissionDiagnostics.isEnabled());
    pacingDiagnostics.configure(submissionDiagnostics.isEnabled());
    configurePreacquire(preacquireSetting);
    return apiVersionSupported() && createInstanceAndSurface() && createDevice() &&
           createSwapchain() && checkDecoderLimits() && createDecoder() && createPlanes() &&
           createPipeline() && createUpscaler() && createFrameResources() && warmUpDecoder() &&
           startWorkers();
}

bool Renderer::check(VkResult result, const char *what) {
    if (result != VK_SUCCESS) {
        LOGE("%s failed: %d", what, result);
        return false;
    }
    return true;
}

bool Renderer::createInstanceAndSurface() {
    if (!vk.loadGlobal()) {
        return false;
    }
    uint32_t loaderVersion = 0;
    if (!check(vk.EnumerateInstanceVersion(&loaderVersion), "Vulkan loader version")) return false;
    if (loaderVersion < VK_API_VERSION_1_1) {
        LOGE("Vulkan loader %u.%u is older than 1.1", VK_API_VERSION_MAJOR(loaderVersion), VK_API_VERSION_MINOR(loaderVersion));
        return false;
    }

    // These create infos stay alive for the device's lifetime: PyroWave reads them.
    appInfo = {VK_STRUCTURE_TYPE_APPLICATION_INFO};
    appInfo.pApplicationName = "Nightjar";
    appInfo.apiVersion = instanceApiVersion(loaderVersion);
    instanceInfo = {VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO};
    instanceInfo.pApplicationInfo = &appInfo;
    uint32_t extensionCount = 0;
    if (!check(vk.EnumerateInstanceExtensionProperties(nullptr, &extensionCount, nullptr), "instance extension count")) return false;
    std::vector<VkExtensionProperties> extensions(extensionCount);
    if (!check(vk.EnumerateInstanceExtensionProperties(nullptr, &extensionCount, extensions.data()), "instance extensions")) return false;
    extensions.resize(extensionCount);
    instanceExtensions.assign(std::begin(INSTANCE_EXTENSIONS), std::end(INSTANCE_EXTENSIONS));
    bool colorspace = false;
    for (const auto &ext : extensions) {
        if (!strcmp(ext.extensionName, VK_EXT_SWAPCHAIN_COLOR_SPACE_EXTENSION_NAME)) colorspace = true;
    }
    if (tenBit && !colorspace) return false;
    if (colorspace) instanceExtensions.push_back(VK_EXT_SWAPCHAIN_COLOR_SPACE_EXTENSION_NAME);
    instanceInfo.enabledExtensionCount = uint32_t(instanceExtensions.size());
    instanceInfo.ppEnabledExtensionNames = instanceExtensions.data();
    if (!check(vk.CreateInstance(&instanceInfo, nullptr, &instance), "vkCreateInstance")) {
        return false;
    }
    if (!vk.loadInstance(instance)) {
        LOGE("Vulkan instance is missing required functions");
        // destroy() must not call through missing pointers.
        auto destroyInstance = reinterpret_cast<PFN_vkDestroyInstance>(vk.GetInstanceProcAddr(instance, "vkDestroyInstance"));
        if (destroyInstance != nullptr) {
            destroyInstance(instance, nullptr);
        }
        instance = VK_NULL_HANDLE;
        return false;
    }

    VkAndroidSurfaceCreateInfoKHR surfaceInfo = {VK_STRUCTURE_TYPE_ANDROID_SURFACE_CREATE_INFO_KHR};
    surfaceInfo.window = window;
    return check(vk.CreateAndroidSurfaceKHR(instance, &surfaceInfo, nullptr, &surface), "vkCreateAndroidSurfaceKHR");
}

bool Renderer::createDevice() {
    uint32_t count = 0;
    if (!check(vk.EnumeratePhysicalDevices(instance, &count, nullptr), "physical device count") || count == 0) return false;
    std::vector<VkPhysicalDevice> devices(count);
    if (!check(vk.EnumeratePhysicalDevices(instance, &count, devices.data()), "physical devices")) return false;
    if (!vk.usingRequestedDriver()) {
        LOGW("Custom Vulkan driver did not load; retry with the system driver");
        return false;
    }
    devices.resize(count);

    FeatureProbe chosenProbe;
    for (auto device : devices) {
        auto probe = probeFeatures(vk, device, tenBit, appInfo.apiVersion);
        if (probe.reason != READY) {
            LOGI("Skipping %s: PyroWave reason %d", probe.name, probe.reason);
            continue;
        }
        uint32_t familyCount = 0;
        vk.GetPhysicalDeviceQueueFamilyProperties(device, &familyCount, nullptr);
        std::vector<VkQueueFamilyProperties> families(familyCount);
        vk.GetPhysicalDeviceQueueFamilyProperties(device, &familyCount, families.data());
        for (uint32_t i = 0; i < familyCount; ++i) {
            VkBool32 presentable = VK_FALSE;
            vk.GetPhysicalDeviceSurfaceSupportKHR(device, i, surface, &presentable);
            if ((families[i].queueFlags & (VK_QUEUE_GRAPHICS_BIT | VK_QUEUE_COMPUTE_BIT)) ==
                (VK_QUEUE_GRAPHICS_BIT | VK_QUEUE_COMPUTE_BIT) && presentable) {
                physicalDevice = device;
                queueFamily = i;
                queueCount = pyroQueueCount(families[i].queueCount, probe.legacySync);
                chosenProbe = probe;
                break;
            }
        }
        if (physicalDevice != VK_NULL_HANDLE) {
            break;
        }
    }
    if (physicalDevice == VK_NULL_HANDLE) {
        LOGE("No Vulkan device can decode PyroWave and present to this surface");
        return false;
    }
    legacySync = chosenProbe.legacySync;
    const uint32_t coreVersion = pyroCoreVersion(chosenProbe.apiVersion, appInfo.apiVersion);
    const bool core12 = coreVersion >= VK_API_VERSION_1_2;

    queuePriorities[0] = queueCount == 2 ? 0.0f : 1.0f;
    queueInfo = {VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO};
    // Like a game's render queue: ask the GPU scheduler to run our work ahead of other apps.
    globalPriority = {VK_STRUCTURE_TYPE_DEVICE_QUEUE_GLOBAL_PRIORITY_CREATE_INFO_EXT};
    globalPriority.globalPriority = VK_QUEUE_GLOBAL_PRIORITY_HIGH_EXT;
    queueInfo.queueFamilyIndex = queueFamily;
    queueInfo.queueCount = queueCount;
    queueInfo.pQueuePriorities = queuePriorities;

    // PyroWave (Granite) reads these structs back from deviceInfo's chain: the core
    // structs where the device has that version, else the extension structs.
    features2 = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_FEATURES_2};
    features2.features.shaderInt16 = VK_TRUE;
    features2.features.shaderStorageImageWriteWithoutFormat = VK_TRUE;
    void **next = &features2.pNext;
    const auto chain = [&next](auto &feature) {
        *next = &feature;
        next = &feature.pNext;
    };
    deviceExtensions.assign(std::begin(DEVICE_EXTENSIONS), std::end(DEVICE_EXTENSIONS));
    if (core12) {
        features11 = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_VULKAN_1_1_FEATURES};
        features11.storageBuffer16BitAccess = VK_TRUE;
        chain(features11);
        features12 = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_VULKAN_1_2_FEATURES};
        features12.timelineSemaphore = legacySync ? VK_FALSE : VK_TRUE;
        features12.storageBuffer8BitAccess = chosenProbe.storage8 ? VK_TRUE : VK_FALSE;
        features12.shaderFloat16 = chosenProbe.float16 ? VK_TRUE : VK_FALSE;
        chain(features12);
    } else {
        storage16Features = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_16BIT_STORAGE_FEATURES};
        storage16Features.storageBuffer16BitAccess = VK_TRUE;
        chain(storage16Features);
        if (chosenProbe.storage8) {
            storage8Features = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_8BIT_STORAGE_FEATURES_KHR};
            storage8Features.storageBuffer8BitAccess = VK_TRUE;
            chain(storage8Features);
        }
        if (chosenProbe.float16) {
            float16Int8Features = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_SHADER_FLOAT16_INT8_FEATURES_KHR};
            float16Int8Features.shaderFloat16 = VK_TRUE;
            chain(float16Int8Features);
        }
    }
    if (legacySync) {
        sizeControlFeatures = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_SUBGROUP_SIZE_CONTROL_FEATURES_EXT};
        sizeControlFeatures.subgroupSizeControl = VK_TRUE;
        sizeControlFeatures.computeFullSubgroups = VK_TRUE;
        chain(sizeControlFeatures);
    } else {
        features13 = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_VULKAN_1_3_FEATURES};
        features13.synchronization2 = VK_TRUE;
        features13.subgroupSizeControl = VK_TRUE;
        features13.computeFullSubgroups = VK_TRUE;
        chain(features13);
    }

    deviceInfo = {VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO};
    deviceInfo.pNext = &features2;
    deviceInfo.queueCreateInfoCount = 1;
    deviceInfo.pQueueCreateInfos = &queueInfo;
    uint32_t extensionCount = 0;
    if (!check(vk.EnumerateDeviceExtensionProperties(physicalDevice, nullptr, &extensionCount, nullptr), "device extension count")) return false;
    std::vector<VkExtensionProperties> extensions(extensionCount);
    if (!check(vk.EnumerateDeviceExtensionProperties(physicalDevice, nullptr, &extensionCount, extensions.data()), "device extensions")) return false;
    extensions.resize(extensionCount);
    for (const char *name : pyroPathExtensions(coreVersion, chosenProbe.storage8, chosenProbe.float16,
                                               hasExtension(extensions, VK_KHR_DRIVER_PROPERTIES_EXTENSION_NAME))) {
        deviceExtensions.push_back(name);
    }
    bool metadata = false;
    bool fifoLatestReadyExtension = false;
    const char *priorityExtension = nullptr;
    for (const auto &ext : extensions) {
        for (const char *name : GLOBAL_PRIORITY_EXTENSIONS) {
            if (priorityExtension == nullptr && !strcmp(ext.extensionName, name)) priorityExtension = name;
        }
        if (!strcmp(ext.extensionName, VK_EXT_HDR_METADATA_EXTENSION_NAME)) metadata = true;
        if (!strcmp(ext.extensionName, VK_GOOGLE_DISPLAY_TIMING_EXTENSION_NAME)) displayTimingSupported = true;
        if (!strcmp(ext.extensionName, VK_EXT_PRESENT_MODE_FIFO_LATEST_READY_EXTENSION_NAME)) fifoLatestReadyExtension = true;
    }
    if (fifoLatestReadyExtension) {
        fifoLatestReadyFeatures = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PRESENT_MODE_FIFO_LATEST_READY_FEATURES_EXT};
        VkPhysicalDeviceFeatures2 available = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_FEATURES_2};
        available.pNext = &fifoLatestReadyFeatures;
        vk.GetPhysicalDeviceFeatures2(physicalDevice, &available);
        if (fifoLatestReadyFeatures.presentModeFifoLatestReady) {
            fifoLatestReadyFeatures.pNext = nullptr;
            chain(fifoLatestReadyFeatures);
            deviceExtensions.push_back(VK_EXT_PRESENT_MODE_FIFO_LATEST_READY_EXTENSION_NAME);
        }
    }
    if (displayTimingSupported) deviceExtensions.push_back(VK_GOOGLE_DISPLAY_TIMING_EXTENSION_NAME);
    // KHR present_id + present_wait provide no timestamp, and can signal for replaced images.
    // Sampling CLOCK_MONOTONIC after a wait would invent a per-frame presentation time.
    if (tenBit && metadata) deviceExtensions.push_back(VK_EXT_HDR_METADATA_EXTENSION_NAME);
    if (priorityExtension != nullptr) {
        deviceExtensions.push_back(priorityExtension);
        queueInfo.pNext = &globalPriority;
    }
    deviceInfo.enabledExtensionCount = uint32_t(deviceExtensions.size());
    deviceInfo.ppEnabledExtensionNames = deviceExtensions.data();
    VkResult created = vk.CreateDevice(physicalDevice, &deviceInfo, nullptr, &device);
    if (created != VK_SUCCESS && priorityExtension != nullptr) {
        // Drivers may refuse elevated priority to unprivileged apps; run at normal priority.
        LOGI("High GPU queue priority refused (%d); using normal priority", created);
        deviceExtensions.pop_back();
        queueInfo.pNext = nullptr;
        deviceInfo.enabledExtensionCount = uint32_t(deviceExtensions.size());
        priorityExtension = nullptr;
        created = vk.CreateDevice(physicalDevice, &deviceInfo, nullptr, &device);
    }
    if (!check(created, "vkCreateDevice")) {
        return false;
    }
    highPriorityQueue = priorityExtension != nullptr;
    if (!vk.loadDevice(device)) {
        LOGE("Vulkan device is missing required functions");
        auto destroyDevice = reinterpret_cast<PFN_vkDestroyDevice>(vk.GetDeviceProcAddr(device, "vkDestroyDevice"));
        if (destroyDevice != nullptr) {
            destroyDevice(device, nullptr);
        }
        device = VK_NULL_HANDLE;
        return false;
    }
    if (!legacySync && vk.WaitSemaphores == nullptr) {
        LOGE("Vulkan device is missing vkWaitSemaphores");
        return false;
    }
    if (tenBit && vk.SetHdrMetadataEXT == nullptr) LOGI("HDR metadata unavailable; HDR is signalled without it");
    displayTimingSupported = displayTimingSupported && vk.GetPastPresentationTimingGOOGLE != nullptr;
    LOGI("Actual presentation timestamps: %s", displayTimingSupported ? "VK_GOOGLE_display_timing" : "unavailable");
    // PyroWave's backend uses queue 0. Prefer a separate higher-priority
    // render queue from the same family, so no ownership transfers are needed.
    vk.GetDeviceQueue(device, queueFamily, 0, &decodeQueue);
    vk.GetDeviceQueue(device, queueFamily, queueCount == 2 ? 1 : 0, &queue);
    LOGI("Decode and presentation queues: %s", queueCount == 2 ? "separate" : "shared");
    VkPhysicalDeviceProperties props;
    vk.GetPhysicalDeviceProperties(physicalDevice, &props);
    VkPhysicalDeviceDriverProperties driverProps = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_DRIVER_PROPERTIES};
    VkPhysicalDeviceProperties2 props2 = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PROPERTIES_2};
    if (core12 || hasExtension(extensions, VK_KHR_DRIVER_PROPERTIES_EXTENSION_NAME)) props2.pNext = &driverProps;
    vk.GetPhysicalDeviceProperties2(physicalDevice, &props2);
    driverDescription = std::string(props.deviceName) + ", " + driverProps.driverName +
                        (driverProps.driverInfo[0] ? std::string(" ") + driverProps.driverInfo : std::string());
    LOGI("GPU driver: %s, queue priority %s", driverDescription.c_str(), highPriorityQueue ? "high" : "normal");

    LOGI("%s on Vulkan %u.%u (float16: %d, 8-bit storage: %d, decode completion: %s)", chosenProbe.name,
         VK_API_VERSION_MAJOR(chosenProbe.apiVersion), VK_API_VERSION_MINOR(chosenProbe.apiVersion),
         chosenProbe.float16, chosenProbe.storage8, legacySync ? "fence" : "timeline");
    return true;
}

bool Renderer::checkDecoderLimits() {
    VkPhysicalDeviceProperties props;
    vk.GetPhysicalDeviceProperties(physicalDevice, &props);
    return props.limits.maxImageDimension2D >= std::max((width + 127) & ~127u, (height + 127) & ~127u);
}

bool Renderer::warmUpDecoder() {
    // Compile and execute before advertising this format. Warm-up never
    // enters the completion/event queues or the stream's timing samples.
    const uint32_t sequence[] = {0x80000000u | (width - 1) | ((height - 1) << 14),
                                 uint32_t(chroma444) << 26};
    const int index = reserveDecodeSlot();
    if (index < 0 || pyrowave_decoder_push_packet(decoder, sequence, sizeof(sequence)) != PYROWAVE_SUCCESS ||
        !pyrowave_decoder_decode_is_ready(decoder, false) || !decodeFrame(index, 0) ||
        !waitForDecode(index)) return false;
    schedule.complete(index, false);
    pyrowave_decoder_clear(decoder);
    decodeStats = {};
    return true;
}

bool Renderer::createDecoder() {
    pyrowave_device_create_queue_info pyroQueue = {decodeQueue, queueFamily, 0};
    // Granite discovers extra queues from this create-info and may fetch
    // their handles itself, even if queue_info lists only one. Expose only
    // queue 0 to keep all SDK uploads/retirement under decodeQueueMutex.
    pyroQueueInfo = queueInfo;
    pyroQueueInfo.queueCount = 1;
    pyroDeviceInfo = deviceInfo;
    pyroDeviceInfo.pQueueCreateInfos = &pyroQueueInfo;
    pyrowave_device_create_info info = {};
    info.GetInstanceProcAddr = vk.GetInstanceProcAddr;
    info.instance = instance;
    info.physical_device = physicalDevice;
    info.device = device;
    info.instance_create_info = &instanceInfo;
    info.device_create_info = &pyroDeviceInfo;
    info.queue_info = &pyroQueue;
    info.queue_info_count = 1;
    // Serialize only queues PyroWave can access. On one-queue devices the
    // presenter uses this same mutex; on two-queue devices it has its own.
    info.queue_lock_callback = [](void *self) { static_cast<Renderer *>(self)->decodeQueueMutex.lock(); };
    info.queue_unlock_callback = [](void *self) { static_cast<Renderer *>(self)->decodeQueueMutex.unlock(); };
    info.userdata = this;
    auto result = pyrowave_create_device(&info, &pyroDevice);
    if (result != PYROWAVE_SUCCESS) {
        LOGE("pyrowave_create_device failed: %d", result);
        return false;
    }
    // Mobile GPUs (Adreno, Mali) decode much faster with PyroWave's fragment path,
    // which runs the inverse DWT in render passes instead of compute shaders.
    fragmentPath = pyrowave_decoder_device_prefers_fragment_path(pyroDevice);
    // Queue 0 supports both graphics and compute. Keep every SDK submission
    // on that queue, including its uploads and frame-context retirement.
    pyrowave_device_set_queue_type(pyroDevice, VK_QUEUE_GRAPHICS_BIT);

    pyrowave_decoder_create_info decoderInfo = {};
    decoderInfo.device = pyroDevice;
    decoderInfo.width = int(width);
    decoderInfo.height = int(height);
    decoderInfo.chroma = chroma444 ? PYROWAVE_CHROMA_SUBSAMPLING_444 : PYROWAVE_CHROMA_SUBSAMPLING_420;
    // The decoder writes the planes (as storage images or render targets) and the
    // CSC pass samples them directly.
    decoderInfo.fragment_path = fragmentPath;
    LOGI("Using the PyroWave %s decode path", fragmentPath ? "fragment" : "compute");
    result = pyrowave_decoder_create(&decoderInfo, &decoder);
    if (result != PYROWAVE_SUCCESS) {
        LOGE("pyrowave_decoder_create failed: %d", result);
        return false;
    }
    return true;
}

bool Renderer::createPlane(Plane &plane, uint32_t planeWidth, uint32_t planeHeight) {
    return createImage(plane, planeWidth, planeHeight, planeFormat(),
                       (fragmentPath ? VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT : VK_IMAGE_USAGE_STORAGE_BIT) |
                       VK_IMAGE_USAGE_SAMPLED_BIT);
}

bool Renderer::createImage(Plane &plane, uint32_t planeWidth, uint32_t planeHeight, VkFormat format, VkImageUsageFlags usage) {
    plane.width = planeWidth;
    plane.height = planeHeight;

    VkImageCreateInfo imageInfo = {VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO};
    imageInfo.imageType = VK_IMAGE_TYPE_2D;
    imageInfo.format = format;
    imageInfo.extent = {planeWidth, planeHeight, 1};
    imageInfo.mipLevels = 1;
    imageInfo.arrayLayers = 1;
    imageInfo.samples = VK_SAMPLE_COUNT_1_BIT;
    imageInfo.tiling = VK_IMAGE_TILING_OPTIMAL;
    imageInfo.usage = usage;
    imageInfo.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
    imageInfo.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;
    if (!check(vk.CreateImage(device, &imageInfo, nullptr, &plane.image), "vkCreateImage")) {
        return false;
    }

    VkMemoryRequirements requirements;
    vk.GetImageMemoryRequirements(device, plane.image, &requirements);
    VkPhysicalDeviceMemoryProperties memoryProps;
    vk.GetPhysicalDeviceMemoryProperties(physicalDevice, &memoryProps);
    uint32_t typeIndex = UINT32_MAX;
    for (uint32_t i = 0; i < memoryProps.memoryTypeCount; ++i) {
        if ((requirements.memoryTypeBits & (1u << i)) &&
            (memoryProps.memoryTypes[i].propertyFlags & VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT)) {
            typeIndex = i;
            break;
        }
    }
    if (typeIndex == UINT32_MAX) {
        LOGE("No device-local memory for a plane image");
        return false;
    }
    VkMemoryAllocateInfo allocInfo = {VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO};
    allocInfo.allocationSize = requirements.size;
    allocInfo.memoryTypeIndex = typeIndex;
    if (!check(vk.AllocateMemory(device, &allocInfo, nullptr, &plane.memory), "vkAllocateMemory") ||
        !check(vk.BindImageMemory(device, plane.image, plane.memory, 0), "vkBindImageMemory")) {
        return false;
    }

    VkImageViewCreateInfo viewInfo = {VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO};
    viewInfo.image = plane.image;
    viewInfo.viewType = VK_IMAGE_VIEW_TYPE_2D;
    viewInfo.format = format;
    viewInfo.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
    return check(vk.CreateImageView(device, &viewInfo, nullptr, &plane.view), "vkCreateImageView");
}


bool Renderer::createPlanes() {
    VkFormatProperties props;
    vk.GetPhysicalDeviceFormatProperties(physicalDevice, planeFormat(), &props);
    VkFormatFeatureFlags required = VK_FORMAT_FEATURE_SAMPLED_IMAGE_BIT |
        VK_FORMAT_FEATURE_SAMPLED_IMAGE_FILTER_LINEAR_BIT |
        (fragmentPath ? VK_FORMAT_FEATURE_COLOR_ATTACHMENT_BIT : VK_FORMAT_FEATURE_STORAGE_IMAGE_BIT);
    if ((props.optimalTilingFeatures & required) != required) return false;
    // The CSC shader samples chroma with normalised coordinates, so full-size
    // (4:4:4) planes need no shader change; its siting offset turns itself off.
    const uint32_t chromaWidth = chroma444 ? width : width / 2;
    const uint32_t chromaHeight = chroma444 ? height : height / 2;
    for (auto &slot : slots) {
        if (!createPlane(slot.planes[0], width, height) ||
            !createPlane(slot.planes[1], chromaWidth, chromaHeight) ||
            !createPlane(slot.planes[2], chromaWidth, chromaHeight)) return false;
    }
    return true;
}

void Renderer::destroy() {
    stopWorkers();
    // The present thread, the clock's only user, has ended.
    presentTiming.setHostSchedule(nullptr, nullptr);
    if (presentClock != nullptr) {
        rp_present_clock_free(presentClock);
        presentClock = nullptr;
    }
    if (device != VK_NULL_HANDLE) {
        vk.DeviceWaitIdle(device);
    }
    // PyroWave objects before the VkDevice they borrow.
    if (decoder != nullptr) {
        pyrowave_decoder_destroy(decoder);
        decoder = nullptr;
    }
    if (pyroDevice != nullptr) {
        pyrowave_device_destroy(pyroDevice);
        pyroDevice = nullptr;
    }
    if (device != VK_NULL_HANDLE) {
        destroySwapchainResources();
        // A fence with an acquire's signal on the way must not be destroyed.
        if (acquireFence != VK_NULL_HANDLE && swapchain != VK_NULL_HANDLE &&
            !imageHold.release([this] { return settleAcquireFence(); })) {
            LOGE("PyroWave acquire fence never signalled before teardown");
        }
        if (swapchain != VK_NULL_HANDLE) vk.DestroySwapchainKHR(device, swapchain, nullptr);
        if (acquireFence != VK_NULL_HANDLE) vk.DestroyFence(device, acquireFence, nullptr);
        if (startupFence != VK_NULL_HANDLE) vk.DestroyFence(device, startupFence, nullptr);
        if (decodeTimeline != VK_NULL_HANDLE) vk.DestroySemaphore(device, decodeTimeline, nullptr);
        if (decodeSemaphore != VK_NULL_HANDLE) vk.DestroySemaphore(device, decodeSemaphore, nullptr);
        for (auto &slot : slots) {
            if (slot.decodeFence != VK_NULL_HANDLE) vk.DestroyFence(device, slot.decodeFence, nullptr);
        }
        destroyWarpResources();
        if (commandPool != VK_NULL_HANDLE) vk.DestroyCommandPool(device, commandPool, nullptr);
        if (drawCommandPool != VK_NULL_HANDLE) vk.DestroyCommandPool(device, drawCommandPool, nullptr);
        if (pipeline != VK_NULL_HANDLE) vk.DestroyPipeline(device, pipeline, nullptr);
        if (upscalePipeline != VK_NULL_HANDLE) vk.DestroyPipeline(device, upscalePipeline, nullptr);
        if (upscaleLayout != VK_NULL_HANDLE) vk.DestroyPipelineLayout(device, upscaleLayout, nullptr);
        if (descriptorPool != VK_NULL_HANDLE) vk.DestroyDescriptorPool(device, descriptorPool, nullptr);
        if (pipelineLayout != VK_NULL_HANDLE) vk.DestroyPipelineLayout(device, pipelineLayout, nullptr);
        if (setLayout != VK_NULL_HANDLE) vk.DestroyDescriptorSetLayout(device, setLayout, nullptr);
        if (sampler != VK_NULL_HANDLE) vk.DestroySampler(device, sampler, nullptr);
        if (renderPass != VK_NULL_HANDLE) vk.DestroyRenderPass(device, renderPass, nullptr);
        for (auto &slot : slots) {
            if (slot.drawFence != VK_NULL_HANDLE) vk.DestroyFence(device, slot.drawFence, nullptr);
            for (auto &plane : slot.planes) {
                if (plane.view != VK_NULL_HANDLE) vk.DestroyImageView(device, plane.view, nullptr);
                if (plane.image != VK_NULL_HANDLE) vk.DestroyImage(device, plane.image, nullptr);
                if (plane.memory != VK_NULL_HANDLE) vk.FreeMemory(device, plane.memory, nullptr);
            }
        }
        vk.DestroyDevice(device, nullptr);
        device = VK_NULL_HANDLE;
    }
    if (instance != VK_NULL_HANDLE) {
        if (surface != VK_NULL_HANDLE) vk.DestroySurfaceKHR(instance, surface, nullptr);
        vk.DestroyInstance(instance, nullptr);
        instance = VK_NULL_HANDLE;
    }
    if (window != nullptr) {
        ANativeWindow_release(window);
        window = nullptr;
    }
}

}  // namespace pyrowave_renderer

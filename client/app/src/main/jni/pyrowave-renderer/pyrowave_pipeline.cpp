// SPDX-License-Identifier: GPL-3.0-only
// Adapted from joemossjr16/artemis-android-pyrowave 387d3a5c.
// PyroWave swapchain, SPIR-V shader modules, graphics/SGSR pipelines and frame resources.

#include "pyrowave_renderer_internal.h"
#include "shaders_spv.h"

namespace pyrowave_renderer {

const uint32_t *warpPlanarCscFragSpv(size_t *sizeBytes) {
    *sizeBytes = sizeof(planar_csc_warp_frag_spv);
    return planar_csc_warp_frag_spv;
}

const uint32_t *warpSgsrFragSpv(size_t *sizeBytes) {
    *sizeBytes = sizeof(sgsr_warp_frag_spv);
    return sgsr_warp_frag_spv;
}

bool Renderer::createSwapchain() {
    // An image taken ahead goes with the old swapchain; its fence must signal before the
    // new swapchain's first acquire uses it.
    if (swapchain != VK_NULL_HANDLE && !imageHold.release([this] { return settleAcquireFence(); })) return false;
    VkSurfaceCapabilitiesKHR caps;
    if (!check(vk.GetPhysicalDeviceSurfaceCapabilitiesKHR(physicalDevice, surface, &caps), "surface capabilities")) {
        return false;
    }
    if (caps.currentExtent.width == 0 || caps.currentExtent.height == 0) {
        LOGW("Surface has no extent yet");
        return false;
    }

    if (swapchainFormat == VK_FORMAT_UNDEFINED) {
        uint32_t formatCount = 0;
        if (!check(vk.GetPhysicalDeviceSurfaceFormatsKHR(physicalDevice, surface, &formatCount, nullptr), "surface format count")) return false;
        std::vector<VkSurfaceFormatKHR> formats(formatCount);
        if (!check(vk.GetPhysicalDeviceSurfaceFormatsKHR(physicalDevice, surface, &formatCount, formats.data()), "surface formats")) return false;
        formats.resize(formatCount);
        if (formats.empty()) {
            LOGE("Surface reports no formats");
            return false;
        }
        surfaceFormats.clear();
        for (const auto &format : formats) {
            surfaceFormats.push_back({int32_t(format.format), int32_t(format.colorSpace)});
        }
        const auto choice = pyroSelectSwapchainFormat(surfaceFormats.data(), surfaceFormats.size(), tenBit, hlg);
        swapchainFormat = static_cast<VkFormat>(choice.format);
        dataspaceOverride = choice.dataspaceOverride;
        if (tenBit && swapchainFormat == VK_FORMAT_UNDEFINED) {
            hdrSurfaceFailure = 1;
            LOGE("PyroWave HDR: the surface lists no usable 10-bit format (%zu formats)", surfaceFormats.size());
        }
        if (tenBit && swapchainFormat != VK_FORMAT_UNDEFINED) logHdrOutput();
        if (swapchainFormat == VK_FORMAT_UNDEFINED) return false;

        uint32_t modeCount = 0;
        if (!check(vk.GetPhysicalDeviceSurfacePresentModesKHR(physicalDevice, surface, &modeCount, nullptr), "present mode count")) return false;
        std::vector<VkPresentModeKHR> modes(modeCount);
        if (!check(vk.GetPhysicalDeviceSurfacePresentModesKHR(physicalDevice, surface, &modeCount, modes.data()), "present modes")) return false;
        modes.resize(modeCount);
        VkPresentModeKHR selectedMode = VK_PRESENT_MODE_FIFO_KHR;
        if (fifoLatestReadyFeatures.presentModeFifoLatestReady &&
            std::find(modes.begin(), modes.end(), VK_PRESENT_MODE_FIFO_LATEST_READY_EXT) != modes.end()) {
            selectedMode = VK_PRESENT_MODE_FIFO_LATEST_READY_EXT;
        } else if (std::find(modes.begin(), modes.end(), VK_PRESENT_MODE_MAILBOX_KHR) != modes.end()) {
            // The newest frame replaces a queued one instead of waiting behind it. With
            // FIFO a stream at the display's own rate keeps the queue full, a constant
            // extra frame or two of delay.
            selectedMode = VK_PRESENT_MODE_MAILBOX_KHR;
        }
        presentMode.store(selectedMode, std::memory_order_relaxed);
    }

    if (!(caps.supportedUsageFlags & VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT)) return false;
    // The Android 13 and 14 loader lists no HDR colour space; HDR then rides on the window dataspace.
    swapchainColorSpace = hdr && !dataspaceOverride ? static_cast<VkColorSpaceKHR>(pyroHdrColorSpace(hlg)) :
                          VK_COLOR_SPACE_SRGB_NONLINEAR_KHR;
    uint32_t imageCount = caps.minImageCount + 1;
    if (caps.maxImageCount > 0) {
        imageCount = std::min(imageCount, caps.maxImageCount);
    }

    VkSwapchainCreateInfoKHR info = {VK_STRUCTURE_TYPE_SWAPCHAIN_CREATE_INFO_KHR};
    info.surface = surface;
    info.minImageCount = imageCount;
    info.imageFormat = swapchainFormat;
    info.imageColorSpace = swapchainColorSpace;
    // Fit in logical display coordinates, then render directly into the display's
    // natural orientation. Android can consume this without a rotation pass.
    const auto transform = pyroPreTransform(caps.currentTransform, caps.supportedTransforms);
    if (transform == 0) {
        LOGE("Surface has no supported rotation or identity transform");
        return false;
    }
    const int32_t rotation = pyroRotationQuarterTurns(transform);
    const auto logicalExtent = surfaceExtent(caps);
    const auto bufferExtent = pyroRotateExtent(logicalExtent, rotation);
    info.imageExtent = {bufferExtent.width, bufferExtent.height};
    info.imageArrayLayers = 1;
    info.imageUsage = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT;
    info.imageSharingMode = VK_SHARING_MODE_EXCLUSIVE;
    info.preTransform = static_cast<VkSurfaceTransformFlagBitsKHR>(transform);
    info.compositeAlpha = (caps.supportedCompositeAlpha & VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR) ?
        VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR : VK_COMPOSITE_ALPHA_INHERIT_BIT_KHR;
    const VkPresentModeKHR activePresentMode = presentMode.load(std::memory_order_relaxed);
    info.presentMode = activePresentMode;
    info.clipped = VK_TRUE;
    info.oldSwapchain = swapchain;

    VkSwapchainKHR newSwapchain = VK_NULL_HANDLE;
    VkResult created = vk.CreateSwapchainKHR(device, &info, nullptr, &newSwapchain);
    if (!check(created, "vkCreateSwapchainKHR")) {
        return false;
    }
    if (swapchain == VK_NULL_HANDLE) {
        LOGI("Present mode: %s (display %.2f Hz, stream %d fps)",
             presentModeName(activePresentMode), displayRefreshRateHz, frameRateHz);
    }
    destroySwapchainResources();
    if (swapchain != VK_NULL_HANDLE) {
        pollRenderedFrames();
        {
            std::lock_guard<std::mutex> lock(eventsMutex);
            // Destroying the swapchain loses its pending timings, not necessarily its frames.
            for (const auto &frame : pendingPresents) {
                renderedFrames.push_back(frame.ptsUs);
                renderedFrames.push_back(0);
            }
        }
        pendingPresents.clear();
        vk.DestroySwapchainKHR(device, swapchain, nullptr);
    }
    swapchain = newSwapchain;
    swapchainExtent = info.imageExtent;
    displayExtent = logicalExtent;
    rotationQuarterTurns = rotation;
    surfaceTransform = caps.currentTransform;
    lastSizeCheckUs = nowUs();
    presentTiming.reset();
    // Rebuilding the swapchain held frames up for a reason that is gone with it: the
    // clock learns the stream's lateness again instead of keeping a delay sized for that.
    if (presentClock != nullptr) rp_present_clock_reset(presentClock);
    lastRefreshCheckUs = 0;
    driverRefreshPeriodNs = 0;
    refreshDisplayClock();
    applyHdrMetadata();
    // The loader sets the window dataspace from the colour space on every swapchain creation.
    if (!applyWindowDataspace()) return false;

    uint32_t count = 0;
    if (!check(vk.GetSwapchainImagesKHR(device, swapchain, &count, nullptr), "swapchain image count") || count == 0) return false;
    swapchainImages.resize(count);
    if (!check(vk.GetSwapchainImagesKHR(device, swapchain, &count, swapchainImages.data()), "swapchain images")) return false;
    swapchainImages.resize(count);
    if (submissionDiagnostics.isEnabled()) {
        LOGI("PyroWave swapchain profile: min=%u max=%u requested=%u actual=%u mode=%s "
             "logical=%ux%u buffer=%ux%u transform=%u format=%u",
             caps.minImageCount, caps.maxImageCount, imageCount, count, presentModeName(activePresentMode),
             displayExtent.width, displayExtent.height, swapchainExtent.width, swapchainExtent.height,
             uint32_t(info.preTransform), uint32_t(swapchainFormat));
    }
    return renderPass == VK_NULL_HANDLE || createSwapchainResources();
}

bool Renderer::createSwapchainResources() {
    for (auto image : swapchainImages) {
        VkImageViewCreateInfo viewInfo = {VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO};
        viewInfo.image = image;
        viewInfo.viewType = VK_IMAGE_VIEW_TYPE_2D;
        viewInfo.format = swapchainFormat;
        viewInfo.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
        VkImageView view;
        if (!check(vk.CreateImageView(device, &viewInfo, nullptr, &view), "swapchain view")) {
            return false;
        }
        swapchainViews.push_back(view);

        VkFramebufferCreateInfo fbInfo = {VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO};
        fbInfo.renderPass = renderPass;
        fbInfo.attachmentCount = 1;
        fbInfo.pAttachments = &view;
        fbInfo.width = swapchainExtent.width;
        fbInfo.height = swapchainExtent.height;
        fbInfo.layers = 1;
        VkFramebuffer framebuffer;
        if (!check(vk.CreateFramebuffer(device, &fbInfo, nullptr, &framebuffer), "vkCreateFramebuffer")) {
            return false;
        }
        framebuffers.push_back(framebuffer);

        // Per image, so a present never waits on a semaphore a later submit re-signals.
        VkSemaphoreCreateInfo semInfo = {VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO};
        VkSemaphore semaphore;
        if (!check(vk.CreateSemaphore(device, &semInfo, nullptr, &semaphore), "vkCreateSemaphore")) {
            return false;
        }
        renderDone.push_back(semaphore);
    }
    return true;
}

void Renderer::destroySwapchainResources() {
    for (auto framebuffer : framebuffers) {
        vk.DestroyFramebuffer(device, framebuffer, nullptr);
    }
    for (auto view : swapchainViews) {
        vk.DestroyImageView(device, view, nullptr);
    }
    for (auto semaphore : renderDone) {
        vk.DestroySemaphore(device, semaphore, nullptr);
    }
    framebuffers.clear();
    swapchainViews.clear();
    renderDone.clear();
}

PyroWaveExtent Renderer::surfaceExtent(const VkSurfaceCapabilitiesKHR &caps) const {
    return {pyroSurfaceDimension(caps.currentExtent.width, ANativeWindow_getWidth(window),
                                 caps.minImageExtent.width, caps.maxImageExtent.width),
            pyroSurfaceDimension(caps.currentExtent.height, ANativeWindow_getHeight(window),
                                 caps.minImageExtent.height, caps.maxImageExtent.height)};
}

// Include same-size rotations (landscape to reverse landscape), which do not
// necessarily resize the Surface or make presentation report SUBOPTIMAL.
bool Renderer::surfaceSizeChanged(uint64_t now) {
    constexpr uint64_t SIZE_CHECK_INTERVAL_US = 250'000;
    if (now - lastSizeCheckUs < SIZE_CHECK_INTERVAL_US) {
        return false;
    }
    lastSizeCheckUs = now;
    VkSurfaceCapabilitiesKHR caps;
    if (vk.GetPhysicalDeviceSurfaceCapabilitiesKHR(physicalDevice, surface, &caps) != VK_SUCCESS) {
        return false;
    }
    const auto extent = surfaceExtent(caps);
    return caps.currentTransform != surfaceTransform || extent.width != displayExtent.width ||
           extent.height != displayExtent.height;
}

bool Renderer::recreateSwapchain() {
    {
        // The two mutexes are always distinct. The single-queue fallback
        // uses decodeQueueMutex for both producers.
        std::scoped_lock lock(decodeQueueMutex, renderQueueMutex);
        if (!check(vk.DeviceWaitIdle(device), "wait before recreating swapchain")) return false;
    }
    return createSwapchain();
}

VkShaderModule Renderer::createShader(const uint32_t *code, size_t size) {
    VkShaderModuleCreateInfo info = {VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO};
    info.codeSize = size;
    info.pCode = code;
    VkShaderModule module = VK_NULL_HANDLE;
    check(vk.CreateShaderModule(device, &info, nullptr, &module), "vkCreateShaderModule");
    return module;
}

bool Renderer::createPipeline() {
    VkAttachmentDescription attachment = {};
    attachment.format = swapchainFormat;
    attachment.samples = VK_SAMPLE_COUNT_1_BIT;
    attachment.loadOp = VK_ATTACHMENT_LOAD_OP_CLEAR;  // Black letterbox bars.
    attachment.storeOp = VK_ATTACHMENT_STORE_OP_STORE;
    attachment.stencilLoadOp = VK_ATTACHMENT_LOAD_OP_DONT_CARE;
    attachment.stencilStoreOp = VK_ATTACHMENT_STORE_OP_DONT_CARE;
    attachment.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;
    attachment.finalLayout = VK_IMAGE_LAYOUT_PRESENT_SRC_KHR;
    VkAttachmentReference colorRef = {0, VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL};
    VkSubpassDescription subpass = {};
    subpass.pipelineBindPoint = VK_PIPELINE_BIND_POINT_GRAPHICS;
    subpass.colorAttachmentCount = 1;
    subpass.pColorAttachments = &colorRef;
    // Order the layout transition after the acquire semaphore wait.
    VkSubpassDependency dependency = {};
    dependency.srcSubpass = VK_SUBPASS_EXTERNAL;
    dependency.dstSubpass = 0;
    dependency.srcStageMask = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
    dependency.dstStageMask = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
    dependency.dstAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT;
    VkRenderPassCreateInfo rpInfo = {VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO};
    rpInfo.attachmentCount = 1;
    rpInfo.pAttachments = &attachment;
    rpInfo.subpassCount = 1;
    rpInfo.pSubpasses = &subpass;
    rpInfo.dependencyCount = 1;
    rpInfo.pDependencies = &dependency;
    if (!check(vk.CreateRenderPass(device, &rpInfo, nullptr, &renderPass), "vkCreateRenderPass")) {
        return false;
    }

    VkSamplerCreateInfo samplerInfo = {VK_STRUCTURE_TYPE_SAMPLER_CREATE_INFO};
    samplerInfo.magFilter = VK_FILTER_LINEAR;
    samplerInfo.minFilter = VK_FILTER_LINEAR;
    samplerInfo.mipmapMode = VK_SAMPLER_MIPMAP_MODE_NEAREST;
    samplerInfo.addressModeU = VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE;
    samplerInfo.addressModeV = VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE;
    samplerInfo.addressModeW = VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE;
    samplerInfo.maxLod = 0.0f;
    if (!check(vk.CreateSampler(device, &samplerInfo, nullptr, &sampler), "vkCreateSampler")) {
        return false;
    }

    VkDescriptorSetLayoutBinding bindings[3] = {};
    for (uint32_t i = 0; i < 3; ++i) {
        bindings[i].binding = i;
        bindings[i].descriptorType = VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
        bindings[i].descriptorCount = 1;
        bindings[i].stageFlags = VK_SHADER_STAGE_FRAGMENT_BIT;
    }
    VkDescriptorSetLayoutCreateInfo layoutInfo = {VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO};
    layoutInfo.bindingCount = 3;
    layoutInfo.pBindings = bindings;
    if (!check(vk.CreateDescriptorSetLayout(device, &layoutInfo, nullptr, &setLayout), "vkCreateDescriptorSetLayout")) {
        return false;
    }
    VkPipelineLayoutCreateInfo plInfo = {VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO};
    plInfo.setLayoutCount = 1;
    plInfo.pSetLayouts = &setLayout;
    VkPushConstantRange colorRange = {VK_SHADER_STAGE_FRAGMENT_BIT, 0, sizeof(PyroWaveColorParams)};
    plInfo.pushConstantRangeCount = 1;
    plInfo.pPushConstantRanges = &colorRange;
    if (!check(vk.CreatePipelineLayout(device, &plInfo, nullptr, &pipelineLayout), "vkCreatePipelineLayout")) {
        return false;
    }

    VkDescriptorPoolSize poolSize = {VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER, 3 * SLOT_COUNT};
    VkDescriptorPoolCreateInfo poolInfo = {VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO};
    poolInfo.maxSets = SLOT_COUNT;
    poolInfo.poolSizeCount = 1;
    poolInfo.pPoolSizes = &poolSize;
    if (!check(vk.CreateDescriptorPool(device, &poolInfo, nullptr, &descriptorPool), "vkCreateDescriptorPool")) {
        return false;
    }
    for (auto &slot : slots) {
        VkDescriptorSetAllocateInfo setInfo = {VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO};
        setInfo.descriptorPool = descriptorPool;
        setInfo.descriptorSetCount = 1;
        setInfo.pSetLayouts = &setLayout;
        if (!check(vk.AllocateDescriptorSets(device, &setInfo, &slot.descriptorSet), "vkAllocateDescriptorSets")) {
            return false;
        }
        // The plane images never change, so the descriptors are written once.
        VkDescriptorImageInfo imageInfos[3];
        VkWriteDescriptorSet writes[3] = {};
        for (uint32_t i = 0; i < 3; ++i) {
            imageInfos[i] = {sampler, slot.planes[i].view, VK_IMAGE_LAYOUT_GENERAL};
            writes[i].sType = VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET;
            writes[i].dstSet = slot.descriptorSet;
            writes[i].dstBinding = i;
            writes[i].descriptorCount = 1;
            writes[i].descriptorType = VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
            writes[i].pImageInfo = &imageInfos[i];
        }
        vk.UpdateDescriptorSets(device, 3, writes, 0, nullptr);
    }

    pipeline = buildPipeline(planar_csc_frag_spv, sizeof(planar_csc_frag_spv), pipelineLayout, renderPass);
    return pipeline != VK_NULL_HANDLE && createSwapchainResources();
}

// A fullscreen-triangle pipeline with dynamic viewport and scissor, writing `colorAttachments`
// colour attachments.
VkPipeline Renderer::buildPipeline(const uint32_t *fragCode, size_t fragSize, VkPipelineLayout layout, VkRenderPass pass,
                         uint32_t colorAttachments) {
    VkShaderModule vert = createShader(fullscreen_vert_spv, sizeof(fullscreen_vert_spv));
    VkShaderModule frag = createShader(fragCode, fragSize);
    if (vert == VK_NULL_HANDLE || frag == VK_NULL_HANDLE) {
        if (vert != VK_NULL_HANDLE) vk.DestroyShaderModule(device, vert, nullptr);
        if (frag != VK_NULL_HANDLE) vk.DestroyShaderModule(device, frag, nullptr);
        return VK_NULL_HANDLE;
    }
    VkPipelineShaderStageCreateInfo stages[2] = {};
    stages[0].sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO;
    stages[0].stage = VK_SHADER_STAGE_VERTEX_BIT;
    stages[0].module = vert;
    stages[0].pName = "main";
    stages[1].sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO;
    stages[1].stage = VK_SHADER_STAGE_FRAGMENT_BIT;
    stages[1].module = frag;
    stages[1].pName = "main";

    VkPipelineVertexInputStateCreateInfo vertexInput = {VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO};
    VkPipelineInputAssemblyStateCreateInfo inputAssembly = {VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO};
    inputAssembly.topology = VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST;
    VkPipelineViewportStateCreateInfo viewportState = {VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO};
    viewportState.viewportCount = 1;
    viewportState.scissorCount = 1;
    VkPipelineRasterizationStateCreateInfo raster = {VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO};
    raster.polygonMode = VK_POLYGON_MODE_FILL;
    raster.cullMode = VK_CULL_MODE_NONE;
    raster.frontFace = VK_FRONT_FACE_COUNTER_CLOCKWISE;
    raster.lineWidth = 1.0f;
    VkPipelineMultisampleStateCreateInfo multisample = {VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO};
    multisample.rasterizationSamples = VK_SAMPLE_COUNT_1_BIT;
    VkPipelineColorBlendAttachmentState blendAttachments[2] = {};
    for (auto &blendAttachment : blendAttachments) {
        blendAttachment.colorWriteMask = VK_COLOR_COMPONENT_R_BIT | VK_COLOR_COMPONENT_G_BIT |
                                         VK_COLOR_COMPONENT_B_BIT | VK_COLOR_COMPONENT_A_BIT;
    }
    VkPipelineColorBlendStateCreateInfo blend = {VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO};
    blend.attachmentCount = std::min(colorAttachments, 2u);
    blend.pAttachments = blendAttachments;
    const VkDynamicState dynamicStates[] = {VK_DYNAMIC_STATE_VIEWPORT, VK_DYNAMIC_STATE_SCISSOR};
    VkPipelineDynamicStateCreateInfo dynamic = {VK_STRUCTURE_TYPE_PIPELINE_DYNAMIC_STATE_CREATE_INFO};
    dynamic.dynamicStateCount = 2;
    dynamic.pDynamicStates = dynamicStates;

    VkGraphicsPipelineCreateInfo pipelineInfo = {VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO};
    pipelineInfo.stageCount = 2;
    pipelineInfo.pStages = stages;
    pipelineInfo.pVertexInputState = &vertexInput;
    pipelineInfo.pInputAssemblyState = &inputAssembly;
    pipelineInfo.pViewportState = &viewportState;
    pipelineInfo.pRasterizationState = &raster;
    pipelineInfo.pMultisampleState = &multisample;
    pipelineInfo.pColorBlendState = &blend;
    pipelineInfo.pDynamicState = &dynamic;
    pipelineInfo.layout = layout;
    pipelineInfo.renderPass = pass;
    VkPipeline created = VK_NULL_HANDLE;
    check(vk.CreateGraphicsPipelines(device, VK_NULL_HANDLE, 1, &pipelineInfo, nullptr, &created),
          "vkCreateGraphicsPipelines");
    vk.DestroyShaderModule(device, vert, nullptr);
    vk.DestroyShaderModule(device, frag, nullptr);
    return created;
}

// Upscaling is optional: any failure leaves the direct CSC path in place.
bool Renderer::createUpscaler() {
    // SGSR works on display-referred SDR; ten-bit output keeps the direct path.
    if (upscaleMode == UPSCALE_SGSR && !tenBit && !createUpscalerResources()) {
        LOGW("SGSR setup failed; presenting directly");
    }
    return true;
}

// SGSR reads the YCbCr planes through the same descriptor set as the direct path and
// converts colour itself, so upscaling costs one pass with no intermediate image.
bool Renderer::createUpscalerResources() {
    VkPushConstantRange range = {VK_SHADER_STAGE_FRAGMENT_BIT, 0, sizeof(PyroWaveSgsrParams)};
    VkPipelineLayoutCreateInfo plInfo = {VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO};
    plInfo.setLayoutCount = 1;
    plInfo.pSetLayouts = &setLayout;
    plInfo.pushConstantRangeCount = 1;
    plInfo.pPushConstantRanges = &range;
    if (!check(vk.CreatePipelineLayout(device, &plInfo, nullptr, &upscaleLayout), "vkCreatePipelineLayout(sgsr)")) return false;
    upscalePipeline = buildPipeline(sgsr_frag_spv, sizeof(sgsr_frag_spv), upscaleLayout, renderPass);
    LOGI("SGSR upscaling %s", upscalePipeline != VK_NULL_HANDLE ? "ready" : "failed");
    return upscalePipeline != VK_NULL_HANDLE;
}

bool Renderer::createFrameResources() {
    VkCommandPoolCreateInfo poolInfo = {VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO};
    poolInfo.flags = VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT;
    poolInfo.queueFamilyIndex = queueFamily;
    if (!check(vk.CreateCommandPool(device, &poolInfo, nullptr, &commandPool), "vkCreateCommandPool")) {
        return false;
    }
    VkCommandBufferAllocateInfo allocInfo = {VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO};
    allocInfo.commandPool = commandPool;
    allocInfo.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY;
    allocInfo.commandBufferCount = 1;
    if (!check(vk.AllocateCommandBuffers(device, &allocInfo, &decodeCommandBuffer), "vkAllocateCommandBuffers(decode)")) {
        return false;
    }
    // Command pools must not be accessed concurrently by the decode and presenter threads.
    if (!check(vk.CreateCommandPool(device, &poolInfo, nullptr, &drawCommandPool), "vkCreateCommandPool(draw)")) {
        return false;
    }
    allocInfo.commandPool = drawCommandPool;
    allocInfo.commandBufferCount = SLOT_COUNT;
    VkCommandBuffer buffers[SLOT_COUNT];
    if (!check(vk.AllocateCommandBuffers(device, &allocInfo, buffers), "vkAllocateCommandBuffers")) {
        return false;
    }
    for (int i = 0; i < SLOT_COUNT; ++i) slots[i].drawCommandBuffer = buffers[i];
    VkFenceCreateInfo fenceInfo = {VK_STRUCTURE_TYPE_FENCE_CREATE_INFO};
    fenceInfo.flags = VK_FENCE_CREATE_SIGNALED_BIT;
    for (auto &slot : slots) {
        if (!check(vk.CreateFence(device, &fenceInfo, nullptr, &slot.drawFence), "vkCreateFence(draw)")) return false;
    }
    VkFenceCreateInfo acquireInfo = {VK_STRUCTURE_TYPE_FENCE_CREATE_INFO};
    if (!check(vk.CreateFence(device, &acquireInfo, nullptr, &acquireFence), "vkCreateFence(acquire)")) return false;
    VkSemaphoreCreateInfo semaphoreInfo = {VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO};
    if (legacySync) {
        if (!check(vk.CreateSemaphore(device, &semaphoreInfo, nullptr, &decodeSemaphore), "vkCreateSemaphore(decode)")) return false;
        for (auto &slot : slots) {
            if (!check(vk.CreateFence(device, &acquireInfo, nullptr, &slot.decodeFence), "vkCreateFence(decode)")) return false;
        }
    } else {
        VkSemaphoreTypeCreateInfo timelineInfo = {VK_STRUCTURE_TYPE_SEMAPHORE_TYPE_CREATE_INFO};
        timelineInfo.semaphoreType = VK_SEMAPHORE_TYPE_TIMELINE;
        semaphoreInfo.pNext = &timelineInfo;
        if (!check(vk.CreateSemaphore(device, &semaphoreInfo, nullptr, &decodeTimeline), "vkCreateSemaphore(decode)")) return false;
    }

    // CAPI-owned commands expect GENERAL images. Transition all plane sets
    // once, before the workers start, then keep that layout throughout.
    VkCommandBufferBeginInfo beginInfo = {VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO};
    beginInfo.flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT;
    if (!check(vk.BeginCommandBuffer(decodeCommandBuffer, &beginInfo), "begin plane initialization")) return false;
    for (const auto &slot : slots) {
        planeBarrier(decodeCommandBuffer, slot, VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT, 0,
                     decodeWriteStage(), decodeWriteAccess(), VK_IMAGE_LAYOUT_UNDEFINED);
    }
    if (!check(vk.EndCommandBuffer(decodeCommandBuffer), "end plane initialization")) return false;
    VkFence initializationFence = VK_NULL_HANDLE;
    if (!check(vk.CreateFence(device, &acquireInfo, nullptr, &initializationFence), "create plane initialization fence")) return false;
    VkSubmitInfo submitInfo = {VK_STRUCTURE_TYPE_SUBMIT_INFO};
    submitInfo.commandBufferCount = 1;
    submitInfo.pCommandBuffers = &decodeCommandBuffer;
    const bool submitted = check(vk.QueueSubmit(decodeQueue, 1, &submitInfo, initializationFence), "initialize plane layouts");
    const bool complete = submitted && check(vk.WaitForFences(device, 1, &initializationFence, VK_TRUE, FENCE_TIMEOUT_NS), "plane initialization fence");
    // A failed wait may still leave work in flight. Teardown waits for the
    // device before destroying this pool; retain the fence there as well.
    if (!complete) {
        startupFence = initializationFence;
        return false;
    }
    vk.DestroyFence(device, initializationFence, nullptr);
    LOGI("Present mode %s, asynchronous decode with up to %d frames in flight",
         presentModeName(presentMode.load(std::memory_order_relaxed)), PyroWaveSchedule::MAX_IN_FLIGHT);
    return true;
}

}  // namespace pyrowave_renderer

// VulkanRenderer swapchain lifecycle, HDR window dataspace / static & HDR10+ metadata,
// and YCbCr sampler conversion management.
// Adapted from MoreOrLessSoftware/moonlight-android (see SOURCE).

#include "vulkan_renderer_internal.h"

namespace vkr {

using namespace renderer_detail;

bool VulkanRenderer::ensureSwapchain() {
    if (!swapchainDirty_ && swapchain_) {
        return true;
    }
    if (!surface_) {
        return false;
    }
    if (!createSwapchain()) {
        return false;
    }
    swapchainDirty_ = false;
    return true;
}

bool VulkanRenderer::createSwapchain() {
    // Only a window found empty below keeps failures from counting; any other failure does
    const bool wasEmpty = outputWindowEmpty_;
    outputWindowEmpty_ = false;
    lastSuboptimalCheckNs_ = 0;
    VkSurfaceCapabilitiesKHR caps;
    if (vk_.vkGetPhysicalDeviceSurfaceCapabilitiesKHR(physicalDevice_, surface_, &caps) != VK_SUCCESS) {
        return false;
    }

    VkExtent2D extent = caps.currentExtent;
    if (extent.width == UINT32_MAX) {
        extent.width = static_cast<uint32_t>(ANativeWindow_getWidth(outputWindow_));
        extent.height = static_cast<uint32_t>(ANativeWindow_getHeight(outputWindow_));
    }
    if (extent.width == 0 || extent.height == 0) {
        // Not a failure: the window is being resized or rotated, and has a size again soon
        if (!wasEmpty) {
            ALOGW("The output window has no size; not drawing until it has");
        }
        outputWindowEmpty_ = true;
        return false;
    }
    const VkSurfaceTransformFlagBitsKHR transform = choosePreTransform(caps);
    extent = preRotatedExtent(extent, transform);

    uint32_t formatCount = 0;
    vk_.vkGetPhysicalDeviceSurfaceFormatsKHR(physicalDevice_, surface_, &formatCount, nullptr);
    std::vector<VkSurfaceFormatKHR> formats(formatCount);
    vk_.vkGetPhysicalDeviceSurfaceFormatsKHR(physicalDevice_, surface_, &formatCount, formats.data());
    if (formats.empty()) {
        return false;
    }

    // The stream is already BT.2020 PQ or HLG, so its HDR passes straight through where the
    // surface takes it (output_format.h)
    std::vector<OutputSurfaceFormat> listed;
    listed.reserve(formats.size());
    for (const auto& f : formats) {
        listed.push_back({static_cast<int32_t>(f.format), static_cast<int32_t>(f.colorSpace)});
    }
    const bool hlg = config_.hdrTransfer == kHdrTransferHlg;
    const OutputChoice choice = vkSelectOutput(listed.data(), listed.size(), hdrActive_, config_.hdrTransfer,
                                               config_.tenBit || hdrActive_);
    VkSurfaceFormatKHR chosen {static_cast<VkFormat>(choice.format), static_cast<VkColorSpaceKHR>(choice.colorSpace)};
    if (choice.toneMapToSdr) {
        ALOGW("%s shown as SDR: %s", hlg ? "HLG" : "HDR10",
              choice.hdrRefusal == HdrRefusal::kNoTenBitFormat ? "the surface lists no ten-bit format" :
                      "the surface lists no colour space for it");
    }

    uint32_t modeCount = 0;
    vk_.vkGetPhysicalDeviceSurfacePresentModesKHR(physicalDevice_, surface_, &modeCount, nullptr);
    std::vector<VkPresentModeKHR> modes(modeCount);
    vk_.vkGetPhysicalDeviceSurfacePresentModesKHR(physicalDevice_, surface_, &modeCount, modes.data());
    VkPresentModeKHR presentMode = VK_PRESENT_MODE_FIFO_KHR;
    const bool shared = hasSharedPresentExt_ &&
            std::find(modes.begin(), modes.end(), VK_PRESENT_MODE_SHARED_CONTINUOUS_REFRESH_KHR) != modes.end();
    if (shared) {
        // The compositor takes the image again at every vsync, so whatever was last drawn into
        // it is shown without presenting a new buffer
        presentMode = VK_PRESENT_MODE_SHARED_CONTINUOUS_REFRESH_KHR;
    }
    else if (pacer_.mode() == PacingMode::MinLatency &&
            std::find(modes.begin(), modes.end(), VK_PRESENT_MODE_MAILBOX_KHR) != modes.end()) {
        presentMode = VK_PRESENT_MODE_MAILBOX_KHR;
    }
    if (hasSharedPresentExt_ && !shared) {
        ALOGW("Front buffer asked for, but the surface has no shared present mode");
    }

    // Frames presented ahead wait in the swapchain until their vsync: up to the jitter buffer
    // plus the present delay's worth of them at once. A shared image is the only one.
    uint32_t imageCount = std::max(caps.minImageCount, presentAhead_ ? 6u : 3u);
    if (caps.maxImageCount != 0) {
        imageCount = std::min(imageCount, caps.maxImageCount);
    }
    if (shared) {
        imageCount = 1;
    }

    VkCompositeAlphaFlagBitsKHR compositeAlpha = VK_COMPOSITE_ALPHA_INHERIT_BIT_KHR;
    if (caps.supportedCompositeAlpha & VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR) {
        compositeAlpha = VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR;
    }
    else if (!(caps.supportedCompositeAlpha & VK_COMPOSITE_ALPHA_INHERIT_BIT_KHR)) {
        compositeAlpha = static_cast<VkCompositeAlphaFlagBitsKHR>(
                caps.supportedCompositeAlpha & -caps.supportedCompositeAlpha);
    }

    VkSwapchainCreateInfoKHR info {VK_STRUCTURE_TYPE_SWAPCHAIN_CREATE_INFO_KHR};
    info.surface = surface_;
    info.minImageCount = imageCount;
    info.imageFormat = chosen.format;
    info.imageColorSpace = chosen.colorSpace;
    info.imageExtent = extent;
    info.imageArrayLayers = 1;
    info.imageUsage = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT;
    info.imageSharingMode = VK_SHARING_MODE_EXCLUSIVE;
    info.preTransform = transform;
    info.compositeAlpha = compositeAlpha;
    info.presentMode = presentMode;
    info.clipped = VK_TRUE;
    info.oldSwapchain = swapchain_;

    // Nothing may still use the old swapchain's images when we destroy them
    waitIdle();

    VkSwapchainKHR swapchain = VK_NULL_HANDLE;
    VkResult result = vk_.vkCreateSwapchainKHR(device_, &info, nullptr, &swapchain);
    destroySwapchain();
    if (result != VK_SUCCESS && shared) {
        // The driver offered the mode but refused it: the normal swapchain from now on
        ALOGW("Front buffer swapchain refused (%d); using the normal swapchain", result);
        hasSharedPresentExt_ = false;
        return createSwapchain();
    }
    if (result != VK_SUCCESS) {
        ALOGE("vkCreateSwapchainKHR failed: %d", result);
        return false;
    }
    swapchain_ = swapchain;
    surfaceFormat_ = chosen;
    presentMode_ = presentMode;
    extent_ = extent;
    preTransform_ = transform;
    // The loader set the window dataspace from the swapchain's colour space just now, so the HDR one
    // goes on after every (re)creation, before the first present
    nwds::DataspaceOutcome dataspace;
    if (choice.dataspaceOverride != 0) {
        dataspace = applyWindowDataspace(choice.dataspaceOverride);
    }
    const bool takesDataspace = choice.dataspaceOverride == 0 || dataspace.accepted();
    outputHdr_ = hdrActive_ && !choice.toneMapToSdr && takesDataspace;
    hdr10PlusDirty_ = true;
    outputDataspace_ = outputHdr_ ? choice.dataspaceOverride : 0;
    hdrToneMapped_ = hdrActive_ && !outputHdr_;
    outputBits_ = chosen.format == VK_FORMAT_A2B10G10R10_UNORM_PACK32 ? 10 : 8;
    frontBuffer_ = shared;
    frontBufferUnavailable_ = frontBufferRequested_ && !shared;
    frontBufferPaced_ = shared && frontBufferAligned_ && presentAhead_;
    presentOnArrival_ = presentMode == VK_PRESENT_MODE_MAILBOX_KHR || (shared && !frontBufferPaced_);
    presentScheduler_.onSwapchainCreated();
    {
        std::lock_guard<std::mutex> lock(mutex_);
        trace_.swapchain(nowNs(), chosen.format, chosen.colorSpace, hdrActive_, extent.width, extent.height,
                         presentMode);
    }

    // A shared image stays in its own layout, in which the display reads it
    const VkImageLayout finalLayout = shared ? VK_IMAGE_LAYOUT_SHARED_PRESENT_KHR : VK_IMAGE_LAYOUT_PRESENT_SRC_KHR;
    if (renderPass_ && (renderPassFormat_ != chosen.format || renderPassFinalLayout_ != finalLayout)) {
        destroyPipelines();
        vk_.vkDestroyRenderPass(device_, renderPass_, nullptr);
        renderPass_ = VK_NULL_HANDLE;
    }
    if (!renderPass_) {
        VkAttachmentDescription color {};
        color.format = chosen.format;
        color.samples = VK_SAMPLE_COUNT_1_BIT;
        color.loadOp = VK_ATTACHMENT_LOAD_OP_DONT_CARE;  // Every pixel is drawn
        color.storeOp = VK_ATTACHMENT_STORE_OP_STORE;
        color.stencilLoadOp = VK_ATTACHMENT_LOAD_OP_DONT_CARE;
        color.stencilStoreOp = VK_ATTACHMENT_STORE_OP_DONT_CARE;
        color.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;
        color.finalLayout = finalLayout;

        VkAttachmentReference ref {0, VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL};
        VkSubpassDescription subpass {};
        subpass.pipelineBindPoint = VK_PIPELINE_BIND_POINT_GRAPHICS;
        subpass.colorAttachmentCount = 1;
        subpass.pColorAttachments = &ref;

        // Wait for the presentation engine to be done with the image before writing it
        VkSubpassDependency dependency {};
        dependency.srcSubpass = VK_SUBPASS_EXTERNAL;
        dependency.dstSubpass = 0;
        dependency.srcStageMask = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
        dependency.dstStageMask = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
        // A shared image is acquired with nothing to wait for, so it also waits for the last
        // frame drawn into it
        dependency.srcAccessMask = shared ? VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT : 0;
        dependency.dstAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT;

        VkRenderPassCreateInfo rpInfo {VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO};
        rpInfo.attachmentCount = 1;
        rpInfo.pAttachments = &color;
        rpInfo.subpassCount = 1;
        rpInfo.pSubpasses = &subpass;
        rpInfo.dependencyCount = 1;
        rpInfo.pDependencies = &dependency;
        if (vk_.vkCreateRenderPass(device_, &rpInfo, nullptr, &renderPass_) != VK_SUCCESS) {
            ALOGE("vkCreateRenderPass failed");
            return false;
        }
        renderPassFormat_ = chosen.format;
        renderPassFinalLayout_ = finalLayout;
    }

    uint32_t count = 0;
    vk_.vkGetSwapchainImagesKHR(device_, swapchain_, &count, nullptr);
    std::vector<VkImage> images(count);
    vk_.vkGetSwapchainImagesKHR(device_, swapchain_, &count, images.data());
    // A driver may cap the swapchain below the images asked for: no longer delay than it holds
    const int delayCap =
            PresentScheduler::maxDelayFor(imageCount, count, caps.minImageCount, deviceApiLevel(), presentAhead_);
    presentScheduler_.setMaxDelay(delayCap);

    for (VkImage image : images) {
        VkImageViewCreateInfo viewInfo {VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO};
        viewInfo.image = image;
        viewInfo.viewType = VK_IMAGE_VIEW_TYPE_2D;
        viewInfo.format = chosen.format;
        viewInfo.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
        VkImageView view = VK_NULL_HANDLE;
        if (vk_.vkCreateImageView(device_, &viewInfo, nullptr, &view) != VK_SUCCESS) {
            return false;
        }
        swapchainViews_.push_back(view);

        VkFramebufferCreateInfo fbInfo {VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO};
        fbInfo.renderPass = renderPass_;
        fbInfo.attachmentCount = 1;
        fbInfo.pAttachments = &view;
        fbInfo.width = extent.width;
        fbInfo.height = extent.height;
        fbInfo.layers = 1;
        VkFramebuffer framebuffer = VK_NULL_HANDLE;
        if (vk_.vkCreateFramebuffer(device_, &fbInfo, nullptr, &framebuffer) != VK_SUCCESS) {
            return false;
        }
        framebuffers_.push_back(framebuffer);

        VkSemaphoreCreateInfo semInfo {VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO};
        VkSemaphore semaphore = VK_NULL_HANDLE;
        if (vk_.vkCreateSemaphore(device_, &semInfo, nullptr, &semaphore) != VK_SUCCESS) {
            return false;
        }
        renderDone_.push_back(semaphore);
    }

    // HLG carries no static metadata (and HDR10's would make the compositor map it as PQ)
    if (outputHdr_ && !hlg) {
        applyHdrMetadata();
    }

    // A change in HDR state also changes the YCbCr model, so drop the old conversion
    {
        std::lock_guard<std::mutex> lock(mutex_);
        char description[128];
        snprintf(description, sizeof(description), "%s%s",
                 outputHdr_ ? (hlg ? "HLG" : "HDR10") : (outputBits_ == 10 ? "10-bit SDR" : "8-bit SDR"),
                 (hdrActive_ && !outputHdr_) ? " (tone mapped)" : "");
        outputDescription_ = description;
        // Which step kept HDR from the screen, for the swapchain line and the problem report
        std::string refused;
        if (hdrActive_ && !outputHdr_) {
            if (choice.hdrRefusal == HdrRefusal::kNoTenBitFormat) {
                refused = "no 10-bit format";
            }
            else if (choice.hdrRefusal == HdrRefusal::kNoHdrColorSpace) {
                refused = hlg ? "no HLG colour space" : "no HDR10 colour space";
            }
            if (choice.dataspaceOverride != 0 && !dataspace.accepted()) {
                char code[128];
                const char* missing = dataspace.publicResult == nwds::kMissing ? " missing" : "";
                if (dataspace.performResult != nwds::kNotCalled) {
                    snprintf(code, sizeof(code), "window dataspace 0x%x refused (set %d%s, perform %d)",
                             static_cast<uint32_t>(choice.dataspaceOverride), dataspace.publicResult, missing,
                             dataspace.performResult);
                }
                else {
                    snprintf(code, sizeof(code), "window dataspace 0x%x refused (set %d%s)",
                             static_cast<uint32_t>(choice.dataspaceOverride), dataspace.publicResult, missing);
                }
                if (!refused.empty()) refused += ", ";
                refused += code;
            }
            if (refused.empty()) refused = "the surface took no HDR output";
        }
        const std::string state = refused.empty() ? description : description + std::string(" (") + refused + ")";
        char swapchain[320];
        snprintf(swapchain, sizeof(swapchain), "%s%s, format %d, color space %d, %u images (min %u), %s, delay cap %d",
                 state.c_str(), outputDataspace_ != 0 ? " (window dataspace)" : "", chosen.format, chosen.colorSpace,
                 count, caps.minImageCount,
                 shared ? "front buffer" : presentMode == VK_PRESENT_MODE_MAILBOX_KHR ? "mailbox" : "FIFO", delayCap);
        swapchainDescription_ = swapchain;
    }

    ALOGI("Swapchain %ux%u, %s, pre-rotated %d degrees", extent.width, extent.height, swapchainDescription_.c_str(),
          quarterTurns(transform) * 90);
    return true;
}

void VulkanRenderer::destroySwapchain() {
    for (VkFramebuffer fb : framebuffers_) {
        vk_.vkDestroyFramebuffer(device_, fb, nullptr);
    }
    for (VkImageView view : swapchainViews_) {
        vk_.vkDestroyImageView(device_, view, nullptr);
    }
    for (VkSemaphore sem : renderDone_) {
        vk_.vkDestroySemaphore(device_, sem, nullptr);
    }
    framebuffers_.clear();
    swapchainViews_.clear();
    renderDone_.clear();
    if (swapchain_) {
        vk_.vkDestroySwapchainKHR(device_, swapchain_, nullptr);
        swapchain_ = VK_NULL_HANDLE;
    }
}

nwds::DataspaceOutcome VulkanRenderer::applyWindowDataspace(int32_t dataspace) {
    nwds::DataspaceOutcome outcome;
    if (dataspace == 0) {
        // Nothing to set (the caller asks only when there is one): the swapchain carries the picture
        outcome.call = nwds::DataspaceCall::kPublic;
        outcome.publicResult = 0;
        return outcome;
    }
    // HLG goes through perform where the public call refuses it, as the window probe that let Java
    // ask for HLG did (window_dataspace.h); PQ keeps the public call, whose refusal means no HDR here.
    auto* setDataSpace = ndk_->ANativeWindow_setBuffersDataSpace;
    outcome = nwds::setDataspace(
            outputWindow_, dataspace, [setDataSpace](void* window, int32_t value) {
                return setDataSpace ? setDataSpace(static_cast<ANativeWindow*>(window), value) : nwds::kMissing;
            }, dataspace == kDataspaceBt2020Hlg);
    if (!outcome.accepted()) {
        // Shown as SDR then, and said on the status line (hdrToneMapped())
        ALOGE("Window dataspace 0x%x was refused (ANativeWindow_setBuffersDataSpace %d%s, perform %d); tone mapping "
              "HDR to SDR", static_cast<uint32_t>(dataspace), outcome.publicResult,
              setDataSpace ? "" : " missing", outcome.performResult);
        return outcome;
    }
    if (outcome.call == nwds::DataspaceCall::kPerform) {
        ALOGI("Window dataspace 0x%x set through ANativeWindow::perform (the public call returned %d)",
              static_cast<uint32_t>(dataspace), outcome.publicResult);
    }
    return outcome;
}

void VulkanRenderer::applyHdrMetadata() {
    if (!hasHdrMetadataExt_ || !swapchain_) {
        ALOGW("Can't pass HDR metadata to the compositor: %s",
              hasHdrMetadataExt_ ? "no swapchain" : "no VK_EXT_hdr_metadata");
        return;
    }
    VkHdrMetadataEXT metadata;
    {
        std::lock_guard<std::mutex> lock(mutex_);
        metadata = hdrMetadata_.vk;
    }
    if (metadata.maxLuminance > 0) {
        vk_.vkSetHdrMetadataEXT(device_, 1, &swapchain_, &metadata);
        ALOGI("Applied HDR metadata: max %.0f nits, MaxCLL %.0f, MaxFALL %.0f", metadata.maxLuminance,
              metadata.maxContentLightLevel, metadata.maxFrameAverageLightLevel);
    }
    else {
        ALOGW("No HDR metadata from the host; the compositor will assume a default peak");
    }
}

void VulkanRenderer::passHdr10Plus(const VideoFrame& frame) {
    // HDR10+ describes PQ pictures only
    const bool pq = outputHdr_ && config_.hdrTransfer == kHdrTransferPq;
    const std::shared_ptr<const std::vector<uint8_t>> wanted = pq ? frame.hdr10Plus : nullptr;
    if (hdr10PlusWindow_.refused() || (wanted == hdr10PlusShown_ && !hdr10PlusDirty_)) {
        return;
    }
    hdr10PlusDirty_ = false;
    hdr10PlusShown_ = wanted;
    const int64_t startNs = nowNs();
    const Hdr10PlusResult result = hdr10PlusWindow_.set(wanted ? wanted->data() : nullptr, wanted ? wanted->size() : 0);
    const int64_t callNs = nowNs() - startNs;
    switch (result) {
        case Hdr10PlusResult::kUnchanged:
            return;
        case Hdr10PlusResult::kSet:
        case Hdr10PlusResult::kCleared:
            if (hdr10PlusCalls_++ == 0) {
                ALOGI("HDR10+ metadata passed to the output window (%zu bytes, %.1f us)", wanted ? wanted->size() : 0,
                      callNs / 1e3);
            }
            hdr10PlusCallNs_ += callNs;
            hdr10PlusMaxCallNs_ = std::max(hdr10PlusMaxCallNs_, callNs);
            if (result == Hdr10PlusResult::kSet) {
                hdr10PlusState_ = kHdr10PlusPassed;
            }
            return;
        case Hdr10PlusResult::kUnsupported:
            ALOGE("HDR10+ not passed on: the output window is not the ANativeWindow this build knows (magic 0x%x, "
                  "version %d, expected %zu); HDR10 static metadata stays", static_cast<uint32_t>(hdr10PlusWindow_.magic()),
                  hdr10PlusWindow_.version(), sizeof(NativeWindowMirror));
            break;
        case Hdr10PlusResult::kFailed:
            ALOGE("HDR10+ not passed on: the output window refused it (%d%s); HDR10 static metadata stays",
                  hdr10PlusWindow_.error(), hdr10PlusWindow_.error() == -ENOENT ? ", unknown to this Android" : "");
            break;
    }
    hdr10PlusShown_ = nullptr;
    hdr10PlusState_ = kHdr10PlusRefused;
}

bool VulkanRenderer::ensureConversion(const ConversionKey& key) {
    if (haveConversion_ && key == conversionKey_) {
        return true;
    }

    waitIdle();
    destroyConversion();

    if (key.ycbcr) {
        VkExternalFormatANDROID externalFormat {VK_STRUCTURE_TYPE_EXTERNAL_FORMAT_ANDROID};
        externalFormat.externalFormat = key.externalFormat;

        VkSamplerYcbcrConversionCreateInfo info {VK_STRUCTURE_TYPE_SAMPLER_YCBCR_CONVERSION_CREATE_INFO};
        info.pNext = key.externalFormat ? &externalFormat : nullptr;
        info.format = key.format;
        info.ycbcrModel = key.model;
        info.ycbcrRange = key.range;
        info.components = key.components;
        info.xChromaOffset = key.xChromaOffset;
        info.yChromaOffset = key.yChromaOffset;
        info.chromaFilter = key.filter;
        info.forceExplicitReconstruction = VK_FALSE;
        VkResult result = vk_.vkCreateSamplerYcbcrConversion(device_, &info, nullptr, &conversion_);
        if (result != VK_SUCCESS) {
            ALOGE("vkCreateSamplerYcbcrConversion failed: %d", result);
            return false;
        }
    }

    VkSamplerYcbcrConversionInfo conversionInfo {VK_STRUCTURE_TYPE_SAMPLER_YCBCR_CONVERSION_INFO};
    conversionInfo.conversion = conversion_;

    VkSamplerCreateInfo samplerInfo {VK_STRUCTURE_TYPE_SAMPLER_CREATE_INFO};
    samplerInfo.pNext = conversion_ ? &conversionInfo : nullptr;
    samplerInfo.magFilter = key.filter;
    samplerInfo.minFilter = key.filter;
    samplerInfo.mipmapMode = VK_SAMPLER_MIPMAP_MODE_NEAREST;
    samplerInfo.addressModeU = VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE;
    samplerInfo.addressModeV = VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE;
    samplerInfo.addressModeW = VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE;
    samplerInfo.maxLod = 0.0f;
    samplerInfo.borderColor = VK_BORDER_COLOR_FLOAT_OPAQUE_BLACK;
    if (vk_.vkCreateSampler(device_, &samplerInfo, nullptr, &sampler_) != VK_SUCCESS) {
        ALOGE("vkCreateSampler failed");
        return false;
    }

    // A YCbCr conversion has to be baked into the layout as an immutable sampler
    VkDescriptorSetLayoutBinding binding {};
    binding.binding = 0;
    binding.descriptorType = VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
    binding.descriptorCount = 1;
    binding.stageFlags = VK_SHADER_STAGE_FRAGMENT_BIT;
    binding.pImmutableSamplers = &sampler_;
    VkDescriptorSetLayoutCreateInfo layoutInfo {VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO};
    layoutInfo.bindingCount = 1;
    layoutInfo.pBindings = &binding;
    if (vk_.vkCreateDescriptorSetLayout(device_, &layoutInfo, nullptr, &setLayout_) != VK_SUCCESS) {
        return false;
    }

    // Multi-planar images can take several descriptors each, so leave room
    VkDescriptorPoolSize poolSize {VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER, (kMaxImports + 8) * 3};
    VkDescriptorPoolCreateInfo poolInfo {VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO};
    poolInfo.flags = VK_DESCRIPTOR_POOL_CREATE_FREE_DESCRIPTOR_SET_BIT;
    poolInfo.maxSets = kMaxImports + 8;
    poolInfo.poolSizeCount = 1;
    poolInfo.pPoolSizes = &poolSize;
    if (vk_.vkCreateDescriptorPool(device_, &poolInfo, nullptr, &descriptorPool_) != VK_SUCCESS) {
        return false;
    }

    VkPushConstantRange range {VK_SHADER_STAGE_VERTEX_BIT | VK_SHADER_STAGE_FRAGMENT_BIT, 0, sizeof(PushConstants)};
    VkPipelineLayoutCreateInfo plInfo {VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO};
    // Set 1 is Frame Warp's HUD mask (video_warp.frag)
    const VkDescriptorSetLayout setLayouts[2] = {setLayout_, warpSetLayout_};
    plInfo.setLayoutCount = warpApi_ ? 2 : 1;
    plInfo.pSetLayouts = setLayouts;
    plInfo.pushConstantRangeCount = 1;
    plInfo.pPushConstantRanges = &range;
    if (vk_.vkCreatePipelineLayout(device_, &plInfo, nullptr, &pipelineLayout_) != VK_SUCCESS) {
        return false;
    }

    conversionKey_ = key;
    haveConversion_ = true;
    ALOGI("YCbCr conversion: external format %" PRIu64 ", format %d, model %d, range %d, filter %d, "
          "components %d %d %d %d", key.externalFormat, key.format, key.model, key.range, key.filter,
          key.components.r, key.components.g, key.components.b, key.components.a);
    return true;
}

void VulkanRenderer::destroyConversion() {
    for (auto& entry : imports_) {
        destroyImport(entry.first, entry.second);
    }
    imports_.clear();

    if (pipeline_) vk_.vkDestroyPipeline(device_, pipeline_, nullptr);
    if (pipelineLayout_) vk_.vkDestroyPipelineLayout(device_, pipelineLayout_, nullptr);
    if (descriptorPool_) vk_.vkDestroyDescriptorPool(device_, descriptorPool_, nullptr);
    if (setLayout_) vk_.vkDestroyDescriptorSetLayout(device_, setLayout_, nullptr);
    if (sampler_) vk_.vkDestroySampler(device_, sampler_, nullptr);
    if (conversion_) vk_.vkDestroySamplerYcbcrConversion(device_, conversion_, nullptr);
    pipeline_ = VK_NULL_HANDLE;
    pipelineLayout_ = VK_NULL_HANDLE;
    descriptorPool_ = VK_NULL_HANDLE;
    setLayout_ = VK_NULL_HANDLE;
    sampler_ = VK_NULL_HANDLE;
    conversion_ = VK_NULL_HANDLE;
    haveConversion_ = false;
}


}  // namespace vkr

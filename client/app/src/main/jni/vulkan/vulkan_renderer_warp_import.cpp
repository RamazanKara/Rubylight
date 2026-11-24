// VulkanRenderer Frame Warp mask resources, graphics pipeline creation, and
// AHardwareBuffer Vulkan import/eviction cache.
// Adapted from MoreOrLessSoftware/moonlight-android (see SOURCE).

#include "vulkan_renderer_internal.h"

namespace vkr {

using namespace renderer_detail;

// ---------------------------------------------------------------------------------------------
// Frame Warp (docs/frame-warp.md)

int32_t VulkanRenderer::findMemoryType(uint32_t typeBits, VkMemoryPropertyFlags flags) const {
    VkPhysicalDeviceMemoryProperties props {};
    vk_.vkGetPhysicalDeviceMemoryProperties(physicalDevice_, &props);
    for (uint32_t i = 0; i < props.memoryTypeCount && i < 32; i++) {
        if ((typeBits & (1u << i)) != 0 && (props.memoryTypes[i].propertyFlags & flags) == flags) {
            return static_cast<int32_t>(i);
        }
    }
    return -1;
}

bool VulkanRenderer::createWarpResources() {
    if (!vk_.vkCreateBuffer || !vk_.vkDestroyBuffer || !vk_.vkGetBufferMemoryRequirements ||
            !vk_.vkBindBufferMemory || !vk_.vkMapMemory || !vk_.vkUnmapMemory || !vk_.vkCmdCopyBufferToImage) {
        ALOGW("Frame Warp: the device lacks the buffer functions its mask needs");
        return false;
    }

    VkShaderModuleCreateInfo shaderInfo {VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO};
    size_t warpSpvSize = 0;
    shaderInfo.pCode = warpFragShaderCode(&warpSpvSize);
    shaderInfo.codeSize = warpSpvSize;
    if (vk_.vkCreateShaderModule(device_, &shaderInfo, nullptr, &warpFragShader_) != VK_SUCCESS) {
        return false;
    }

    // One mask cell covers many pixels: nearest, so its edges stay where the host put them
    VkSamplerCreateInfo samplerInfo {VK_STRUCTURE_TYPE_SAMPLER_CREATE_INFO};
    samplerInfo.magFilter = VK_FILTER_NEAREST;
    samplerInfo.minFilter = VK_FILTER_NEAREST;
    samplerInfo.mipmapMode = VK_SAMPLER_MIPMAP_MODE_NEAREST;
    samplerInfo.addressModeU = VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE;
    samplerInfo.addressModeV = VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE;
    samplerInfo.addressModeW = VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE;
    samplerInfo.maxLod = 0.0f;
    samplerInfo.borderColor = VK_BORDER_COLOR_FLOAT_OPAQUE_BLACK;
    if (vk_.vkCreateSampler(device_, &samplerInfo, nullptr, &warpSampler_) != VK_SUCCESS) {
        return false;
    }

    VkDescriptorSetLayoutBinding binding {};
    binding.binding = 0;
    binding.descriptorType = VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
    binding.descriptorCount = 1;
    binding.stageFlags = VK_SHADER_STAGE_FRAGMENT_BIT;
    binding.pImmutableSamplers = &warpSampler_;
    VkDescriptorSetLayoutCreateInfo layoutInfo {VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO};
    layoutInfo.bindingCount = 1;
    layoutInfo.pBindings = &binding;
    if (vk_.vkCreateDescriptorSetLayout(device_, &layoutInfo, nullptr, &warpSetLayout_) != VK_SUCCESS) {
        return false;
    }

    VkDescriptorPoolSize poolSize {VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER, static_cast<uint32_t>(kFramesInFlight)};
    VkDescriptorPoolCreateInfo poolInfo {VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO};
    poolInfo.maxSets = static_cast<uint32_t>(kFramesInFlight);
    poolInfo.poolSizeCount = 1;
    poolInfo.pPoolSizes = &poolSize;
    if (vk_.vkCreateDescriptorPool(device_, &poolInfo, nullptr, &warpPool_) != VK_SUCCESS) {
        return false;
    }

    // A mask per frame in flight, each written while its slot's fence says the GPU is done with it
    for (WarpMask& mask : warpMasks_) {
        VkDescriptorSetAllocateInfo allocInfo {VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO};
        allocInfo.descriptorPool = warpPool_;
        allocInfo.descriptorSetCount = 1;
        allocInfo.pSetLayouts = &warpSetLayout_;
        if (vk_.vkAllocateDescriptorSets(device_, &allocInfo, &mask.set) != VK_SUCCESS ||
                !createWarpMask(mask, 1, 1)) {
            return false;
        }
    }
    return true;
}

void VulkanRenderer::destroyWarpResources() {
    for (WarpMask& mask : warpMasks_) {
        destroyWarpMask(mask);
        // Freed with the pool
        mask.set = VK_NULL_HANDLE;
    }
    if (warpPool_) vk_.vkDestroyDescriptorPool(device_, warpPool_, nullptr);
    if (warpSetLayout_) vk_.vkDestroyDescriptorSetLayout(device_, warpSetLayout_, nullptr);
    if (warpSampler_) vk_.vkDestroySampler(device_, warpSampler_, nullptr);
    if (warpFragShader_) vk_.vkDestroyShaderModule(device_, warpFragShader_, nullptr);
    warpPool_ = VK_NULL_HANDLE;
    warpSetLayout_ = VK_NULL_HANDLE;
    warpSampler_ = VK_NULL_HANDLE;
    warpFragShader_ = VK_NULL_HANDLE;
}

// Makes the mask's image (R8, width by height cells) and the host-visible buffer it is written
// from, and points the mask's descriptor set at the image. What a failure left is for
// destroyWarpMask(); the set is only changed once everything else was made.
bool VulkanRenderer::createWarpMask(WarpMask& mask, uint32_t width, uint32_t height) {
    VkImageCreateInfo imageInfo {VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO};
    imageInfo.imageType = VK_IMAGE_TYPE_2D;
    imageInfo.format = VK_FORMAT_R8_UNORM;
    imageInfo.extent = {width, height, 1};
    imageInfo.mipLevels = 1;
    imageInfo.arrayLayers = 1;
    imageInfo.samples = VK_SAMPLE_COUNT_1_BIT;
    imageInfo.tiling = VK_IMAGE_TILING_OPTIMAL;
    imageInfo.usage = VK_IMAGE_USAGE_SAMPLED_BIT | VK_IMAGE_USAGE_TRANSFER_DST_BIT;
    imageInfo.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
    imageInfo.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;
    if (vk_.vkCreateImage(device_, &imageInfo, nullptr, &mask.image) != VK_SUCCESS) {
        return false;
    }
    VkMemoryRequirements imageNeeds {};
    vk_.vkGetImageMemoryRequirements(device_, mask.image, &imageNeeds);
    int32_t type = findMemoryType(imageNeeds.memoryTypeBits, VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT);
    if (type < 0) {
        type = findMemoryType(imageNeeds.memoryTypeBits, 0);
    }
    if (type < 0) {
        return false;
    }
    VkMemoryAllocateInfo allocInfo {VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO};
    allocInfo.allocationSize = imageNeeds.size;
    allocInfo.memoryTypeIndex = static_cast<uint32_t>(type);
    if (vk_.vkAllocateMemory(device_, &allocInfo, nullptr, &mask.memory) != VK_SUCCESS ||
            vk_.vkBindImageMemory(device_, mask.image, mask.memory, 0) != VK_SUCCESS) {
        return false;
    }
    VkImageViewCreateInfo viewInfo {VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO};
    viewInfo.image = mask.image;
    viewInfo.viewType = VK_IMAGE_VIEW_TYPE_2D;
    viewInfo.format = VK_FORMAT_R8_UNORM;
    viewInfo.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
    if (vk_.vkCreateImageView(device_, &viewInfo, nullptr, &mask.view) != VK_SUCCESS) {
        return false;
    }

    const VkDeviceSize bytes = static_cast<VkDeviceSize>(width) * height;
    VkBufferCreateInfo bufferInfo {VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO};
    bufferInfo.size = bytes;
    bufferInfo.usage = VK_BUFFER_USAGE_TRANSFER_SRC_BIT;
    bufferInfo.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
    if (vk_.vkCreateBuffer(device_, &bufferInfo, nullptr, &mask.staging) != VK_SUCCESS) {
        return false;
    }
    VkMemoryRequirements bufferNeeds {};
    vk_.vkGetBufferMemoryRequirements(device_, mask.staging, &bufferNeeds);
    // Coherent, so the writes are the GPU's at the submit without a flush
    type = findMemoryType(bufferNeeds.memoryTypeBits,
                          VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT);
    if (type < 0) {
        return false;
    }
    allocInfo.allocationSize = bufferNeeds.size;
    allocInfo.memoryTypeIndex = static_cast<uint32_t>(type);
    if (vk_.vkAllocateMemory(device_, &allocInfo, nullptr, &mask.stagingMemory) != VK_SUCCESS ||
            vk_.vkBindBufferMemory(device_, mask.staging, mask.stagingMemory, 0) != VK_SUCCESS ||
            vk_.vkMapMemory(device_, mask.stagingMemory, 0, VK_WHOLE_SIZE, 0, &mask.mapped) != VK_SUCCESS) {
        mask.mapped = nullptr;
        return false;
    }
    memset(mask.mapped, 0, static_cast<size_t>(bytes));
    mask.width = width;
    mask.height = height;
    mask.generation = kWarpMaskUnwritten;
    mask.writing = kWarpMaskUnwritten;

    // The sampler is the layout's (immutable)
    VkDescriptorImageInfo imageDescriptor {};
    imageDescriptor.imageView = mask.view;
    imageDescriptor.imageLayout = VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL;
    VkWriteDescriptorSet write {VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET};
    write.dstSet = mask.set;
    write.dstBinding = 0;
    write.descriptorCount = 1;
    write.descriptorType = VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
    write.pImageInfo = &imageDescriptor;
    vk_.vkUpdateDescriptorSets(device_, 1, &write, 0, nullptr);
    return true;
}

// Everything but the descriptor set, which stays the mask's
void VulkanRenderer::destroyWarpMask(WarpMask& mask) {
    if (mask.mapped) vk_.vkUnmapMemory(device_, mask.stagingMemory);
    if (mask.staging) vk_.vkDestroyBuffer(device_, mask.staging, nullptr);
    if (mask.stagingMemory) vk_.vkFreeMemory(device_, mask.stagingMemory, nullptr);
    if (mask.view) vk_.vkDestroyImageView(device_, mask.view, nullptr);
    if (mask.image) vk_.vkDestroyImage(device_, mask.image, nullptr);
    if (mask.memory) vk_.vkFreeMemory(device_, mask.memory, nullptr);
    const VkDescriptorSet set = mask.set;
    mask = WarpMask {};
    mask.set = set;
}

// The slot's fence was waited on (renderFrame()), so the GPU is done with its mask and buffer.
// Records a write of the newest mask, or of an empty one, when the slot's image holds another;
// it becomes the image's once the commands are submitted (writing to generation).
int32_t VulkanRenderer::recordWarpMask(int slot, VkCommandBuffer cmd) {
    WarpMask& mask = warpMasks_[slot];
    const bool cells = warpCells_.any() && !warpMaskFailed_;
    const int32_t target = cells ? warpCells_.generation : kWarpMaskEmpty;
    mask.writing = mask.generation;
    if (mask.generation == target) {
        return target;
    }
    if (cells && (mask.width != warpCells_.width || mask.height != warpCells_.height)) {
        WarpMask resized;
        resized.set = mask.set;
        if (createWarpMask(resized, warpCells_.width, warpCells_.height)) {
            destroyWarpMask(mask);
            mask = resized;
        }
        else {
            ALOGW("Frame Warp: no %ux%u mask image; the HUD is shifted with the picture",
                  warpCells_.width, warpCells_.height);
            destroyWarpMask(resized);
            warpMaskFailed_ = true;
            return recordWarpMask(slot, cmd);
        }
    }

    if (cells) {
        memcpy(mask.mapped, warpCells_.cells.data(), static_cast<size_t>(mask.width) * mask.height);
    }
    else {
        memset(mask.mapped, 0, static_cast<size_t>(mask.width) * mask.height);
    }
    VkImageMemoryBarrier toWrite {VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER};
    // Its last reads were in this slot's last commands, which the fence covered
    toWrite.srcAccessMask = 0;
    toWrite.dstAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
    toWrite.oldLayout = VK_IMAGE_LAYOUT_UNDEFINED;
    toWrite.newLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL;
    toWrite.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
    toWrite.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
    toWrite.image = mask.image;
    toWrite.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
    vk_.vkCmdPipelineBarrier(cmd, VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT,
                             0, 0, nullptr, 0, nullptr, 1, &toWrite);
    VkBufferImageCopy region {};
    region.imageSubresource = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1};
    region.imageExtent = {mask.width, mask.height, 1};
    vk_.vkCmdCopyBufferToImage(cmd, mask.staging, mask.image, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 1, &region);
    VkImageMemoryBarrier toRead = toWrite;
    toRead.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
    toRead.dstAccessMask = VK_ACCESS_SHADER_READ_BIT;
    toRead.oldLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL;
    toRead.newLayout = VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL;
    vk_.vkCmdPipelineBarrier(cmd, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT,
                             0, 0, nullptr, 0, nullptr, 1, &toRead);
    mask.writing = target;
    return target;
}

bool VulkanRenderer::ensurePipeline() {
    if (pipeline_) {
        return true;
    }
    return createPipeline(pipelineLayout_, warpApi_ ? warpFragShader_ : fragShader_, &pipeline_);
}

void VulkanRenderer::destroyPipelines() {
    if (pipeline_) {
        vk_.vkDestroyPipeline(device_, pipeline_, nullptr);
        pipeline_ = VK_NULL_HANDLE;
    }
}

bool VulkanRenderer::createPipeline(VkPipelineLayout layout, VkShaderModule fragShader, VkPipeline* pipeline) {
    if (!renderPass_ || !layout) {
        return false;
    }

    VkPipelineShaderStageCreateInfo stages[2] {};
    stages[0].sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO;
    stages[0].stage = VK_SHADER_STAGE_VERTEX_BIT;
    stages[0].module = vertShader_;
    stages[0].pName = "main";
    stages[1].sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO;
    stages[1].stage = VK_SHADER_STAGE_FRAGMENT_BIT;
    stages[1].module = fragShader;
    stages[1].pName = "main";

    VkPipelineVertexInputStateCreateInfo vertexInput {VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO};
    VkPipelineInputAssemblyStateCreateInfo inputAssembly {VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO};
    inputAssembly.topology = VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST;

    VkPipelineViewportStateCreateInfo viewportState {VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO};
    viewportState.viewportCount = 1;
    viewportState.scissorCount = 1;

    VkPipelineRasterizationStateCreateInfo raster {VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO};
    raster.polygonMode = VK_POLYGON_MODE_FILL;
    raster.cullMode = VK_CULL_MODE_NONE;
    raster.frontFace = VK_FRONT_FACE_COUNTER_CLOCKWISE;
    raster.lineWidth = 1.0f;

    VkPipelineMultisampleStateCreateInfo multisample {VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO};
    multisample.rasterizationSamples = VK_SAMPLE_COUNT_1_BIT;

    VkPipelineColorBlendAttachmentState blendAttachment {};
    blendAttachment.colorWriteMask = VK_COLOR_COMPONENT_R_BIT | VK_COLOR_COMPONENT_G_BIT |
                                     VK_COLOR_COMPONENT_B_BIT | VK_COLOR_COMPONENT_A_BIT;
    VkPipelineColorBlendStateCreateInfo blend {VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO};
    blend.attachmentCount = 1;
    blend.pAttachments = &blendAttachment;

    const VkDynamicState dynamicStates[] = {VK_DYNAMIC_STATE_VIEWPORT, VK_DYNAMIC_STATE_SCISSOR};
    VkPipelineDynamicStateCreateInfo dynamic {VK_STRUCTURE_TYPE_PIPELINE_DYNAMIC_STATE_CREATE_INFO};
    dynamic.dynamicStateCount = 2;
    dynamic.pDynamicStates = dynamicStates;

    VkGraphicsPipelineCreateInfo info {VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO};
    info.stageCount = 2;
    info.pStages = stages;
    info.pVertexInputState = &vertexInput;
    info.pInputAssemblyState = &inputAssembly;
    info.pViewportState = &viewportState;
    info.pRasterizationState = &raster;
    info.pMultisampleState = &multisample;
    info.pColorBlendState = &blend;
    info.pDynamicState = &dynamic;
    info.layout = layout;
    info.renderPass = renderPass_;
    info.subpass = 0;

    VkResult result = vk_.vkCreateGraphicsPipelines(device_, VK_NULL_HANDLE, 1, &info, nullptr, pipeline);
    if (result != VK_SUCCESS) {
        ALOGE("vkCreateGraphicsPipelines failed: %d", result);
        *pipeline = VK_NULL_HANDLE;
        return false;
    }
    return true;
}

VulkanRenderer::ImportedBuffer* VulkanRenderer::importBuffer(AHardwareBuffer* buffer) {
    VkAndroidHardwareBufferFormatPropertiesANDROID formatProps {VK_STRUCTURE_TYPE_ANDROID_HARDWARE_BUFFER_FORMAT_PROPERTIES_ANDROID};
    VkAndroidHardwareBufferPropertiesANDROID props {VK_STRUCTURE_TYPE_ANDROID_HARDWARE_BUFFER_PROPERTIES_ANDROID};
    props.pNext = &formatProps;

    // The color model can change with HDR, so the key is checked even for known buffers
    auto it = imports_.find(buffer);
    if (it == imports_.end() || !haveConversion_) {
        VkResult result = vk_.vkGetAndroidHardwareBufferPropertiesANDROID(device_, buffer, &props);
        if (result != VK_SUCCESS) {
            ALOGE("vkGetAndroidHardwareBufferPropertiesANDROID failed: %d", result);
            return nullptr;
        }

        ConversionKey key;
        key.format = formatProps.format;
        key.externalFormat = formatProps.format == VK_FORMAT_UNDEFINED ? formatProps.externalFormat : 0;
        key.ycbcr = key.externalFormat != 0 || isYcbcrFormat(formatProps.format);
        key.components = formatProps.samplerYcbcrConversionComponents;
        key.xChromaOffset = formatProps.suggestedXChromaOffset;
        key.yChromaOffset = formatProps.suggestedYChromaOffset;
        key.filter = (!key.ycbcr || (formatProps.formatFeatures & VK_FORMAT_FEATURE_SAMPLED_IMAGE_YCBCR_CONVERSION_LINEAR_FILTER_BIT))
                     ? VK_FILTER_LINEAR : VK_FILTER_NEAREST;

        // We know how the host encoded the stream, so use that instead of what the buffer
        // suggests. Sunshine sends HDR as BT.2020.
        int colorspace = hdrActive_ ? 2 : config_.colorspace;
        key.model = colorspace == 0 ? VK_SAMPLER_YCBCR_MODEL_CONVERSION_YCBCR_601 :
                    colorspace == 2 ? VK_SAMPLER_YCBCR_MODEL_CONVERSION_YCBCR_2020 :
                                      VK_SAMPLER_YCBCR_MODEL_CONVERSION_YCBCR_709;
        key.range = config_.fullRange ? VK_SAMPLER_YCBCR_RANGE_ITU_FULL : VK_SAMPLER_YCBCR_RANGE_ITU_NARROW;
        if (!key.ycbcr) {
            key.components = {};
        }
        else {
            // The chroma of Tegra's P010 buffers, swapped back (ycbcr_components.h)
            key.components = vkr::ycbcrComponents(key.components, vendorId_, key.format);
        }

        if (!ensureConversion(key)) {
            return nullptr;
        }
        // A new conversion drops every import, including this buffer's
        it = imports_.find(buffer);
    }
    else {
        const int colorspace = hdrActive_ ? 2 : config_.colorspace;
        const VkSamplerYcbcrModelConversion model =
                colorspace == 0 ? VK_SAMPLER_YCBCR_MODEL_CONVERSION_YCBCR_601 :
                colorspace == 2 ? VK_SAMPLER_YCBCR_MODEL_CONVERSION_YCBCR_2020 :
                                  VK_SAMPLER_YCBCR_MODEL_CONVERSION_YCBCR_709;
        if (conversionKey_.ycbcr && conversionKey_.model != model) {
            ConversionKey key = conversionKey_;
            key.model = model;
            if (!ensureConversion(key)) {
                return nullptr;
            }
            it = imports_.find(buffer);
        }
    }

    if (it != imports_.end()) {
        it->second.lastUsed = frameCounter_;
        return &it->second;
    }

    if (props.allocationSize == 0) {
        VkResult result = vk_.vkGetAndroidHardwareBufferPropertiesANDROID(device_, buffer, &props);
        if (result != VK_SUCCESS) {
            ALOGE("vkGetAndroidHardwareBufferPropertiesANDROID failed: %d", result);
            return nullptr;
        }
    }

    AHardwareBuffer_Desc desc {};
    ndk_->AHardwareBuffer_describe(buffer, &desc);

    ImportedBuffer imported;
    imported.width = desc.width;
    imported.height = desc.height;
    imported.lastUsed = frameCounter_;

    VkExternalFormatANDROID externalFormat {VK_STRUCTURE_TYPE_EXTERNAL_FORMAT_ANDROID};
    externalFormat.externalFormat = conversionKey_.externalFormat;
    VkExternalMemoryImageCreateInfo externalMemory {VK_STRUCTURE_TYPE_EXTERNAL_MEMORY_IMAGE_CREATE_INFO};
    externalMemory.pNext = &externalFormat;
    externalMemory.handleTypes = VK_EXTERNAL_MEMORY_HANDLE_TYPE_ANDROID_HARDWARE_BUFFER_BIT_ANDROID;

    VkImageCreateInfo imageInfo {VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO};
    imageInfo.pNext = &externalMemory;
    imageInfo.imageType = VK_IMAGE_TYPE_2D;
    imageInfo.format = conversionKey_.format;
    imageInfo.extent = {desc.width, desc.height, 1};
    imageInfo.mipLevels = 1;
    imageInfo.arrayLayers = 1;
    imageInfo.samples = VK_SAMPLE_COUNT_1_BIT;
    imageInfo.tiling = VK_IMAGE_TILING_OPTIMAL;
    imageInfo.usage = VK_IMAGE_USAGE_SAMPLED_BIT;
    imageInfo.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
    imageInfo.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;
    VkResult result = vk_.vkCreateImage(device_, &imageInfo, nullptr, &imported.image);
    if (result != VK_SUCCESS) {
        ALOGE("vkCreateImage for hardware buffer failed: %d", result);
        return nullptr;
    }

    VkImportAndroidHardwareBufferInfoANDROID importInfo {VK_STRUCTURE_TYPE_IMPORT_ANDROID_HARDWARE_BUFFER_INFO_ANDROID};
    importInfo.buffer = buffer;
    VkMemoryDedicatedAllocateInfo dedicated {VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO};
    dedicated.pNext = &importInfo;
    dedicated.image = imported.image;
    VkMemoryAllocateInfo allocInfo {VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO};
    allocInfo.pNext = &dedicated;
    allocInfo.allocationSize = props.allocationSize;
    allocInfo.memoryTypeIndex = static_cast<uint32_t>(__builtin_ctz(props.memoryTypeBits));
    result = vk_.vkAllocateMemory(device_, &allocInfo, nullptr, &imported.memory);
    if (result != VK_SUCCESS) {
        ALOGE("Importing hardware buffer failed: %d", result);
        vk_.vkDestroyImage(device_, imported.image, nullptr);
        return nullptr;
    }
    vk_.vkBindImageMemory(device_, imported.image, imported.memory, 0);

    VkSamplerYcbcrConversionInfo conversionInfo {VK_STRUCTURE_TYPE_SAMPLER_YCBCR_CONVERSION_INFO};
    conversionInfo.conversion = conversion_;
    VkImageViewCreateInfo viewInfo {VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO};
    viewInfo.pNext = conversion_ ? &conversionInfo : nullptr;
    viewInfo.image = imported.image;
    viewInfo.viewType = VK_IMAGE_VIEW_TYPE_2D;
    viewInfo.format = conversionKey_.format;
    viewInfo.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
    result = vk_.vkCreateImageView(device_, &viewInfo, nullptr, &imported.view);
    if (result != VK_SUCCESS) {
        ALOGE("vkCreateImageView for hardware buffer failed: %d", result);
        vk_.vkFreeMemory(device_, imported.memory, nullptr);
        vk_.vkDestroyImage(device_, imported.image, nullptr);
        return nullptr;
    }

    VkDescriptorSetAllocateInfo setInfo {VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO};
    setInfo.descriptorPool = descriptorPool_;
    setInfo.descriptorSetCount = 1;
    setInfo.pSetLayouts = &setLayout_;
    result = vk_.vkAllocateDescriptorSets(device_, &setInfo, &imported.descriptorSet);
    if (result != VK_SUCCESS) {
        ALOGE("vkAllocateDescriptorSets failed: %d", result);
        vk_.vkDestroyImageView(device_, imported.view, nullptr);
        vk_.vkFreeMemory(device_, imported.memory, nullptr);
        vk_.vkDestroyImage(device_, imported.image, nullptr);
        return nullptr;
    }

    VkDescriptorImageInfo imageDescriptor {};
    imageDescriptor.imageView = imported.view;
    imageDescriptor.imageLayout = VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL;
    VkWriteDescriptorSet write {VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET};
    write.dstSet = imported.descriptorSet;
    write.dstBinding = 0;
    write.descriptorCount = 1;
    write.descriptorType = VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
    write.pImageInfo = &imageDescriptor;
    vk_.vkUpdateDescriptorSets(device_, 1, &write, 0, nullptr);

    // Holding a reference keeps the buffer, and so this cache key, from being reused
    ndk_->AHardwareBuffer_acquire(buffer);
    return &imports_.emplace(buffer, imported).first->second;
}

void VulkanRenderer::destroyImport(AHardwareBuffer* buffer, ImportedBuffer& imported) {
    if (imported.descriptorSet) vk_.vkFreeDescriptorSets(device_, descriptorPool_, 1, &imported.descriptorSet);
    if (imported.view) vk_.vkDestroyImageView(device_, imported.view, nullptr);
    if (imported.image) vk_.vkDestroyImage(device_, imported.image, nullptr);
    if (imported.memory) vk_.vkFreeMemory(device_, imported.memory, nullptr);
    ndk_->AHardwareBuffer_release(buffer);
}

void VulkanRenderer::evictImports() {
    // Buffers the decoder stopped using. The last few frames may still be on the GPU.
    while (imports_.size() > kMaxImports) {
        auto oldest = imports_.end();
        for (auto it = imports_.begin(); it != imports_.end(); ++it) {
            if (it->second.lastUsed + kFramesInFlight + 1 < frameCounter_ &&
                    (oldest == imports_.end() || it->second.lastUsed < oldest->second.lastUsed)) {
                oldest = it;
            }
        }
        if (oldest == imports_.end()) {
            return;
        }
        destroyImport(oldest->first, oldest->second);
        imports_.erase(oldest);
    }
}


}  // namespace vkr

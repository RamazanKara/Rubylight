// SPDX-License-Identifier: GPL-3.0-only
// Adapted from joemossjr16/artemis-android-pyrowave 387d3a5c.
// PyroWave Frame Warp HUD mask staging, shift uniforms, warp pipelines and redraws.

#include "pyrowave_renderer_internal.h"

namespace pyrowave_renderer {

// Frame Warp for this stream (docs/frame-warp.md), with the table `api`. Java calls it once the
// stream offers it, after create() started the workers: everything is made here before the
// table is published, and the present thread draws with it from its next picture on. False,
// with nothing left behind, when the device cannot.
bool Renderer::enableFrameWarp(const FrameWarpApi *api) {
    std::lock_guard<std::mutex> lock(warpSetupMutex);
    if (warpApi.load(std::memory_order_acquire) != nullptr) return true;
    if (device == VK_NULL_HANDLE || renderPass == VK_NULL_HANDLE || failed) return false;
    if (!createWarpResources()) {
        LOGW("Frame Warp could not be set up for PyroWave");
        destroyWarpResources();
        return false;
    }
    warpApi.store(api, std::memory_order_release);
    LOGI("Frame Warp on%s", warpUpscalePipeline != VK_NULL_HANDLE ? ", with SGSR" : "");
    return true;
}

bool Renderer::warpRedraws() const {
    return displayTimingSupported || presentMode.load(std::memory_order_relaxed) == VK_PRESENT_MODE_MAILBOX_KHR;
}

// Frame Warp, present thread: whether to wake at the next vsync for a redraw rather than sleep
// until a new picture. Only with a locked model (the picture on screen was drawn with an active
// shift): the vsync after each new picture, and after that only while the view turns
// (FrameWarpDrawn::mayMove()). A still view, an unlocked model or a stream without input wakes
// the thread for pictures only.
bool Renderer::warpWantsVsync() const {
    const FrameWarpApi *warp = warpApi.load(std::memory_order_acquire);
    return warp != nullptr && warpDrawn.active && (warpLookAfterPicture || warpDrawn.mayMove(warp));
}

// The first vsync after `now` as the present timing knows the display (from an earlier
// vsync and the refresh period), or a refresh period on when it knows no vsync yet.
int64_t Renderer::nextWarpVsync(int64_t now) const {
    const int64_t period = presentTiming.periodNs() > 0 ? presentTiming.periodNs() : 16'666'667;
    const int64_t phase = presentTiming.phaseNs();
    if (phase <= 0) return now + period;
    int64_t remainder = (now - phase) % period;
    if (remainder < 0) remainder += period;
    return now + period - remainder;
}

// Frame Warp: the shift for slot `index`'s picture as of now and the lens terms the rest of
// it is turned through, counted as drawn, into `out` (FRAME_WARP_FLOATS floats,
// frame_warp_api.h) and the uniform its recorded draw reads (the slot's draw fence has
// signalled, so the GPU is not reading it). The HUD stays in place only where the host's mask
// for this shift is the one the draw reads (`mask`). Returns warp()'s code; `seq` gets the
// input count read before it, so input sent meanwhile shows as new at the next vsync.
int32_t Renderer::setWarpShift(int index, const FrameWarpApi *warp, int32_t mask, float *out, uint32_t *seq) {
    WarpSlot &slot = warpSlots[index];
    *seq = warp->input_seq();
    const int32_t code = warp->warp(slot.frameNumber, out);
    if (code == FRAME_WARP_INACTIVE) frameWarpStill(out);
    slot.shiftMapped[0] = out[0];
    slot.shiftMapped[1] = out[1];
    slot.shiftMapped[2] = code >= 0 && code == mask ? 1.0f : 0.0f;
    slot.shiftMapped[3] = 0.0f;
    // vec4 lens
    std::memcpy(slot.shiftMapped + 4, out + 4, 4 * sizeof(float));
    return code;
}

// Frame Warp: slot `index` reached the screen with shift `out`, from warp() code `code` with
// input count `seq` (setWarpShift()); `newPicture` unless it was a redraw. Where redraws can be
// replaced, it is kept to draw again until a newer picture is ready (PyroWaveSchedule::hold).
// `dueNs` is the vsync a paced present asked for, else 0.
void Renderer::warpShown(int index, const float *out, int32_t code, uint32_t seq, int64_t dueNs, bool newPicture) {
    warpDrawn.note(code, out, seq);
    if (newPicture) warpLookAfterPicture = true;
    warpLastPresentNs = nowNs();
    warpDueNs = dueNs;
    if (!warpRedraws()) return;
    std::lock_guard<std::mutex> lock(slotMutex);
    schedule.hold(index);
}

// Frame Warp between the host's pictures (as VulkanRenderer::warpRefresh()): the picture on
// screen, slot `index` (PyroWaveSchedule::beginRedraw), presented again with the shift as of
// now when it moved by kFrameWarpRedrawPx or more since it was drawn. Nothing is drawn when a
// present was queued since this vsync, before the vsync a paced present asked for, while the
// slot's last draw is still on the GPU, or with no swapchain image free now; the next vsync
// tries again. Stamped with the time it is presented, so a new picture due at the same vsync
// replaces it. It is no new frame: no release event, statistics or pacing feedback. Each costs
// one draw, only while the view turns. Present thread; false when the renderer failed.
bool Renderer::redrawSlot(int index, int64_t vsyncNs) {
    const FrameWarpApi *warp = warpApi.load(std::memory_order_acquire);
    if (warp == nullptr || warpLastPresentNs >= vsyncNs || warpDueNs > vsyncNs + presentTiming.periodNs() / 4) {
        return true;
    }
    // The vsync after the new picture is looked at now; from here on, only while the view turns
    warpLookAfterPicture = false;
    // Without the stream's lock first: nothing can have moved unless the view turns
    if (!warpDrawn.mayMove(warp)) return true;
    Slot &slot = slots[index];
    // Never wait for the GPU here
    const VkResult drawn = vk.GetFenceStatus(device, slot.drawFence);
    if (drawn == VK_NOT_READY) return true;
    if (!check(drawn, "Frame Warp draw fence")) return false;
    // Peeked, so a vsync that draws nothing is not counted; setWarpShift() takes what is drawn
    float peeked[FRAME_WARP_FLOATS] = {};
    const bool active = warp->peek(warpSlots[index].frameNumber, peeked) != FRAME_WARP_INACTIVE;
    if (!frameWarpMoved(warpDrawn.px[0], warpDrawn.px[1], active ? peeked[2] : 0.0f, active ? peeked[3] : 0.0f)) {
        return true;
    }
    if (!applyPendingHdr()) return false;
    uint32_t imageIndex = 0;
    const PyroAcquire acquired = imageHold.take(&imageIndex,
            [this](uint32_t *image) { return acquireImage(image, 0); },
            [this] { return settleAcquireFence(); });
    if (acquired == PyroAcquire::OUT_OF_DATE) return recreateSwapchain();
    if (acquired == PyroAcquire::NOT_READY) return true;
    if (acquired != PyroAcquire::READY) return false;

    int32_t warpMask = FrameWarpCells::kEmpty;
    if (!recordDraw(index, imageIndex, warp, &warpMask)) return false;
    float warpOut[FRAME_WARP_FLOATS];
    frameWarpStill(warpOut);
    uint32_t warpSeq = 0;
    const int32_t warpCode = setWarpShift(index, warp, warpMask, warpOut, &warpSeq);

    VkCommandBuffer cmd = slot.drawCommandBuffer;
    // As presentSlot(): the decode it waits for is long done
    VkTimelineSemaphoreSubmitInfo timelineInfo = {VK_STRUCTURE_TYPE_TIMELINE_SEMAPHORE_SUBMIT_INFO};
    timelineInfo.waitSemaphoreValueCount = 1;
    timelineInfo.pWaitSemaphoreValues = &slot.decodeValue;
    const uint64_t binarySignalValue = 0;
    timelineInfo.signalSemaphoreValueCount = 1;
    timelineInfo.pSignalSemaphoreValues = &binarySignalValue;
    const VkPipelineStageFlags waitStage = VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT;
    VkSubmitInfo submitInfo = {VK_STRUCTURE_TYPE_SUBMIT_INFO};
    if (!legacySync) {
        submitInfo.pNext = &timelineInfo;
        submitInfo.waitSemaphoreCount = 1;
        submitInfo.pWaitSemaphores = &decodeTimeline;
        submitInfo.pWaitDstStageMask = &waitStage;
    }
    submitInfo.commandBufferCount = 1;
    submitInfo.pCommandBuffers = &cmd;
    submitInfo.signalSemaphoreCount = 1;
    submitInfo.pSignalSemaphores = &renderDone[imageIndex];
    if (!check(vk.ResetFences(device, 1, &slot.drawFence), "reset draw fence")) return false;
    VkPresentInfoKHR presentInfo = {VK_STRUCTURE_TYPE_PRESENT_INFO_KHR};
    presentInfo.waitSemaphoreCount = 1;
    presentInfo.pWaitSemaphores = &renderDone[imageIndex];
    presentInfo.swapchainCount = 1;
    presentInfo.pSwapchains = &swapchain;
    presentInfo.pImageIndices = &imageIndex;
    VkPresentTimeGOOGLE presentTime = {0, 0};
    VkPresentTimesInfoGOOGLE presentTimes = {VK_STRUCTURE_TYPE_PRESENT_TIMES_INFO_GOOGLE};
    if (displayTimingSupported) {
        if (++nextPresentId == 0) ++nextPresentId;
        presentTime.presentID = nextPresentId;
        // A mailbox replaces it anyway
        presentTime.desiredPresentTime = presentMode.load(std::memory_order_relaxed) == VK_PRESENT_MODE_MAILBOX_KHR ? 0 : uint64_t(nowNs());
        presentTimes.swapchainCount = 1;
        presentTimes.pTimes = &presentTime;
        presentInfo.pNext = &presentTimes;
    }
    VkResult presented = VK_SUCCESS;
    {
        std::lock_guard<std::mutex> lock(queueCount == 2 ? renderQueueMutex : decodeQueueMutex);
        if (!check(vk.QueueSubmit(queue, 1, &submitInfo, slot.drawFence), "vkQueueSubmit(Frame Warp)")) return false;
        presented = vk.QueuePresentKHR(queue, &presentInfo);
    }
    warpSlots[index].generation = warpSlots[index].writing;
    const bool shown = presented == VK_SUCCESS || presented == VK_SUBOPTIMAL_KHR;
    if (shown) warpShown(index, warpOut, warpCode, warpSeq, 0, false);
    if (presented == VK_ERROR_OUT_OF_DATE_KHR) return recreateSwapchain();
    if (shown && preacquireImages) {
        imageHold.preacquire([this](uint32_t *image) { return acquireImage(image, 0); });
    }
    return shown || check(presented, "vkQueuePresentKHR(Frame Warp)");
}

// Frame Warp: the slot's draw fence has signalled, so the GPU is done with its mask and staging
// buffer. Records a write of the newest mask, or of an empty one, when the slot's image holds
// another; it is the image's once the commands are submitted (writing to generation). Returns
// the mask generation the draw reads.
int32_t Renderer::recordWarpMask(int index, VkCommandBuffer cmd) {
    WarpSlot &slot = warpSlots[index];
    const bool cells = warpCells.any() && !warpMaskFailed;
    const int32_t target = cells ? warpCells.generation : FrameWarpCells::kEmpty;
    slot.writing = slot.generation;
    if (slot.generation == target) return target;
    if (cells && (slot.mask.width != warpCells.width || slot.mask.height != warpCells.height) &&
        !replaceWarpMask(slot, warpCells.width, warpCells.height)) {
        LOGW("Frame Warp: no %ux%u mask image; the HUD is shifted with the picture",
             warpCells.width, warpCells.height);
        warpMaskFailed = true;
        return recordWarpMask(index, cmd);
    }
    const size_t bytes = size_t(slot.mask.width) * slot.mask.height;
    if (cells) {
        std::memcpy(slot.mask.mapped, warpCells.cells.data(), bytes);
    } else {
        std::memset(slot.mask.mapped, 0, bytes);
    }
    VkImageMemoryBarrier toWrite = {VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER};
    // Its last reads were in this slot's last commands, which the fence covered
    toWrite.srcAccessMask = 0;
    toWrite.dstAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
    toWrite.oldLayout = VK_IMAGE_LAYOUT_UNDEFINED;
    toWrite.newLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL;
    toWrite.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
    toWrite.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
    toWrite.image = slot.mask.image;
    toWrite.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
    vk.CmdPipelineBarrier(cmd, VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT,
                          0, 0, nullptr, 0, nullptr, 1, &toWrite);
    VkBufferImageCopy region = {};
    region.imageSubresource = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1};
    region.imageExtent = {slot.mask.width, slot.mask.height, 1};
    warpVk.CmdCopyBufferToImage(cmd, slot.mask.staging, slot.mask.image, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
                                1, &region);
    VkImageMemoryBarrier toRead = toWrite;
    toRead.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
    toRead.dstAccessMask = VK_ACCESS_SHADER_READ_BIT;
    toRead.oldLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL;
    toRead.newLayout = VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL;
    vk.CmdPipelineBarrier(cmd, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT,
                          0, 0, nullptr, 0, nullptr, 1, &toRead);
    slot.writing = target;
    return target;
}

// Frame Warp: a `width` by `height` mask for `slot` in place of the one it has, which goes once
// the new one exists (its draws are done). The slot keeps its mask when this fails.
bool Renderer::replaceWarpMask(WarpSlot &slot, uint32_t width, uint32_t height) {
    WarpMask made;
    if (!createWarpMask(made, width, height)) {
        destroyWarpMask(made);
        return false;
    }
    destroyWarpMask(slot.mask);
    slot.mask = made;
    slot.generation = WARP_MASK_UNWRITTEN;
    slot.writing = WARP_MASK_UNWRITTEN;
    writeWarpDescriptors(slot);
    return true;
}

// The first memory type in `bits` with all of `flags`, or -1.
int32_t Renderer::warpMemoryType(uint32_t bits, VkMemoryPropertyFlags flags) {
    VkPhysicalDeviceMemoryProperties properties;
    vk.GetPhysicalDeviceMemoryProperties(physicalDevice, &properties);
    for (uint32_t i = 0; i < properties.memoryTypeCount; ++i) {
        if ((bits & (1u << i)) && (properties.memoryTypes[i].propertyFlags & flags) == flags) return int32_t(i);
    }
    return -1;
}

// A buffer of `size` bytes for `usage` in host-visible, coherent memory, mapped: what the CPU
// writes is the GPU's at the next submit without a flush.
bool Renderer::createMappedBuffer(VkDeviceSize size, VkBufferUsageFlags usage, VkBuffer *buffer, VkDeviceMemory *memory,
                        void **mapped) {
    VkBufferCreateInfo bufferInfo = {VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO};
    bufferInfo.size = size;
    bufferInfo.usage = usage;
    bufferInfo.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
    if (!check(warpVk.CreateBuffer(device, &bufferInfo, nullptr, buffer), "vkCreateBuffer(Frame Warp)")) return false;
    VkMemoryRequirements needs;
    warpVk.GetBufferMemoryRequirements(device, *buffer, &needs);
    const int32_t type = warpMemoryType(needs.memoryTypeBits,
                                        VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT);
    if (type < 0) {
        LOGE("Frame Warp: no host-visible coherent memory");
        return false;
    }
    VkMemoryAllocateInfo allocInfo = {VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO};
    allocInfo.allocationSize = needs.size;
    allocInfo.memoryTypeIndex = uint32_t(type);
    if (!check(vk.AllocateMemory(device, &allocInfo, nullptr, memory), "vkAllocateMemory(Frame Warp)") ||
        !check(warpVk.BindBufferMemory(device, *buffer, *memory, 0), "vkBindBufferMemory(Frame Warp)")) return false;
    if (!check(warpVk.MapMemory(device, *memory, 0, VK_WHOLE_SIZE, 0, mapped), "vkMapMemory(Frame Warp)")) {
        *mapped = nullptr;
        return false;
    }
    std::memset(*mapped, 0, size_t(size));
    return true;
}

// Frame Warp: a mask image of `width` by `height` cells and its staging buffer, all clear.
// What exists of it on failure is left for destroyWarpMask().
bool Renderer::createWarpMask(WarpMask &mask, uint32_t width, uint32_t height) {
    VkImageCreateInfo imageInfo = {VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO};
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
    if (!check(vk.CreateImage(device, &imageInfo, nullptr, &mask.image), "vkCreateImage(Frame Warp)")) return false;
    VkMemoryRequirements needs;
    vk.GetImageMemoryRequirements(device, mask.image, &needs);
    int32_t type = warpMemoryType(needs.memoryTypeBits, VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT);
    if (type < 0) type = warpMemoryType(needs.memoryTypeBits, 0);
    if (type < 0) return false;
    VkMemoryAllocateInfo allocInfo = {VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO};
    allocInfo.allocationSize = needs.size;
    allocInfo.memoryTypeIndex = uint32_t(type);
    if (!check(vk.AllocateMemory(device, &allocInfo, nullptr, &mask.memory), "vkAllocateMemory(Frame Warp)") ||
        !check(vk.BindImageMemory(device, mask.image, mask.memory, 0), "vkBindImageMemory(Frame Warp)")) return false;
    VkImageViewCreateInfo viewInfo = {VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO};
    viewInfo.image = mask.image;
    viewInfo.viewType = VK_IMAGE_VIEW_TYPE_2D;
    viewInfo.format = VK_FORMAT_R8_UNORM;
    viewInfo.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
    if (!check(vk.CreateImageView(device, &viewInfo, nullptr, &mask.view), "vkCreateImageView(Frame Warp)")) return false;
    if (!createMappedBuffer(VkDeviceSize(width) * height, VK_BUFFER_USAGE_TRANSFER_SRC_BIT, &mask.staging,
                            &mask.stagingMemory, &mask.mapped)) return false;
    mask.width = width;
    mask.height = height;
    return true;
}

void Renderer::destroyWarpMask(WarpMask &mask) {
    if (mask.mapped != nullptr) warpVk.UnmapMemory(device, mask.stagingMemory);
    if (mask.staging != VK_NULL_HANDLE) warpVk.DestroyBuffer(device, mask.staging, nullptr);
    if (mask.stagingMemory != VK_NULL_HANDLE) vk.FreeMemory(device, mask.stagingMemory, nullptr);
    if (mask.view != VK_NULL_HANDLE) vk.DestroyImageView(device, mask.view, nullptr);
    if (mask.image != VK_NULL_HANDLE) vk.DestroyImage(device, mask.image, nullptr);
    if (mask.memory != VK_NULL_HANDLE) vk.FreeMemory(device, mask.memory, nullptr);
    mask = WarpMask();
}

// Points the slot's set at its mask image and shift buffer.
void Renderer::writeWarpDescriptors(const WarpSlot &slot) {
    const VkDescriptorImageInfo image = {warpSampler, slot.mask.view, VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL};
    const VkDescriptorBufferInfo shift = {slot.shift, 0, WARP_SHIFT_FLOATS * sizeof(float)};
    VkWriteDescriptorSet writes[2] = {};
    writes[0].sType = VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET;
    writes[0].dstSet = slot.set;
    writes[0].dstBinding = 0;
    writes[0].descriptorCount = 1;
    writes[0].descriptorType = VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
    writes[0].pImageInfo = &image;
    writes[1].sType = VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET;
    writes[1].dstSet = slot.set;
    writes[1].dstBinding = 1;
    writes[1].descriptorCount = 1;
    writes[1].descriptorType = VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER;
    writes[1].pBufferInfo = &shift;
    vk.UpdateDescriptorSets(device, 2, writes, 0, nullptr);
}

// Frame Warp's pipelines (the direct and SGSR passes built with -DFRAME_WARP, which also take
// set 1), and for each slot a clear 1x1 mask, a shift buffer and the set that reads them.
// On Java's thread, before enableFrameWarp() publishes any of it; the workers create nothing.
bool Renderer::createWarpResources() {
#define WARP_LOAD(name) \
    warpVk.name = reinterpret_cast<PFN_vk##name>(vk.GetDeviceProcAddr(device, "vk" #name)); \
    if (warpVk.name == nullptr) return false;
    WARP_LOAD(CreateBuffer)
    WARP_LOAD(DestroyBuffer)
    WARP_LOAD(GetBufferMemoryRequirements)
    WARP_LOAD(BindBufferMemory)
    WARP_LOAD(MapMemory)
    WARP_LOAD(UnmapMemory)
    WARP_LOAD(CmdCopyBufferToImage)
#undef WARP_LOAD

    // The mask's cells, unfiltered
    VkSamplerCreateInfo samplerInfo = {VK_STRUCTURE_TYPE_SAMPLER_CREATE_INFO};
    samplerInfo.magFilter = VK_FILTER_NEAREST;
    samplerInfo.minFilter = VK_FILTER_NEAREST;
    samplerInfo.mipmapMode = VK_SAMPLER_MIPMAP_MODE_NEAREST;
    samplerInfo.addressModeU = VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE;
    samplerInfo.addressModeV = VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE;
    samplerInfo.addressModeW = VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE;
    samplerInfo.maxLod = 0.0f;
    if (!check(vk.CreateSampler(device, &samplerInfo, nullptr, &warpSampler), "vkCreateSampler(Frame Warp)")) return false;

    VkDescriptorSetLayoutBinding bindings[2] = {};
    bindings[0].binding = 0;
    bindings[0].descriptorType = VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
    bindings[0].descriptorCount = 1;
    bindings[0].stageFlags = VK_SHADER_STAGE_FRAGMENT_BIT;
    bindings[1].binding = 1;
    bindings[1].descriptorType = VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER;
    bindings[1].descriptorCount = 1;
    bindings[1].stageFlags = VK_SHADER_STAGE_FRAGMENT_BIT;
    VkDescriptorSetLayoutCreateInfo layoutInfo = {VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO};
    layoutInfo.bindingCount = 2;
    layoutInfo.pBindings = bindings;
    if (!check(vk.CreateDescriptorSetLayout(device, &layoutInfo, nullptr, &warpSetLayout),
               "vkCreateDescriptorSetLayout(Frame Warp)")) return false;

    // Set 0 and the push constants as the passes without Frame Warp have them
    const VkDescriptorSetLayout setLayouts[2] = {setLayout, warpSetLayout};
    VkPipelineLayoutCreateInfo plInfo = {VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO};
    plInfo.setLayoutCount = 2;
    plInfo.pSetLayouts = setLayouts;
    const VkPushConstantRange colorRange = {VK_SHADER_STAGE_FRAGMENT_BIT, 0, sizeof(PyroWaveColorParams)};
    plInfo.pushConstantRangeCount = 1;
    plInfo.pPushConstantRanges = &colorRange;
    if (!check(vk.CreatePipelineLayout(device, &plInfo, nullptr, &warpLayout), "vkCreatePipelineLayout(Frame Warp)")) {
        return false;
    }
    size_t warpCscSize = 0;
    const uint32_t *warpCscCode = warpPlanarCscFragSpv(&warpCscSize);
    warpPipeline = buildPipeline(warpCscCode, warpCscSize, warpLayout, renderPass);
    if (warpPipeline == VK_NULL_HANDLE) return false;
    if (upscalePipeline != VK_NULL_HANDLE) {
        const VkPushConstantRange sgsrRange = {VK_SHADER_STAGE_FRAGMENT_BIT, 0, sizeof(PyroWaveSgsrParams)};
        plInfo.pPushConstantRanges = &sgsrRange;
        if (!check(vk.CreatePipelineLayout(device, &plInfo, nullptr, &warpUpscaleLayout),
                   "vkCreatePipelineLayout(Frame Warp SGSR)")) return false;
        size_t warpSgsrSize = 0;
        const uint32_t *warpSgsrCode = warpSgsrFragSpv(&warpSgsrSize);
        warpUpscalePipeline = buildPipeline(warpSgsrCode, warpSgsrSize, warpUpscaleLayout, renderPass);
        if (warpUpscalePipeline == VK_NULL_HANDLE) return false;
    }

    const VkDescriptorPoolSize poolSizes[2] = {{VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER, SLOT_COUNT},
                                               {VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER, SLOT_COUNT}};
    VkDescriptorPoolCreateInfo poolInfo = {VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO};
    poolInfo.maxSets = SLOT_COUNT;
    poolInfo.poolSizeCount = 2;
    poolInfo.pPoolSizes = poolSizes;
    if (!check(vk.CreateDescriptorPool(device, &poolInfo, nullptr, &warpPool), "vkCreateDescriptorPool(Frame Warp)")) {
        return false;
    }
    for (auto &slot : warpSlots) {
        VkDescriptorSetAllocateInfo setInfo = {VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO};
        setInfo.descriptorPool = warpPool;
        setInfo.descriptorSetCount = 1;
        setInfo.pSetLayouts = &warpSetLayout;
        if (!check(vk.AllocateDescriptorSets(device, &setInfo, &slot.set), "vkAllocateDescriptorSets(Frame Warp)") ||
            !createWarpMask(slot.mask, 1, 1)) return false;
        void *shift = nullptr;
        if (!createMappedBuffer(WARP_SHIFT_FLOATS * sizeof(float), VK_BUFFER_USAGE_UNIFORM_BUFFER_BIT, &slot.shift, &slot.shiftMemory,
                                &shift)) return false;
        slot.shiftMapped = static_cast<float *>(shift);
        // Unwritten, so the first draw clears the mask before it is read
        slot.generation = WARP_MASK_UNWRITTEN;
        slot.writing = WARP_MASK_UNWRITTEN;
        writeWarpDescriptors(slot);
    }
    return true;
}

// Everything createWarpResources() made. The GPU is done with it: it was never published, or
// the device is idle (destroy()).
void Renderer::destroyWarpResources() {
    for (auto &slot : warpSlots) {
        destroyWarpMask(slot.mask);
        if (slot.shiftMapped != nullptr) warpVk.UnmapMemory(device, slot.shiftMemory);
        if (slot.shift != VK_NULL_HANDLE) warpVk.DestroyBuffer(device, slot.shift, nullptr);
        if (slot.shiftMemory != VK_NULL_HANDLE) vk.FreeMemory(device, slot.shiftMemory, nullptr);
        slot = WarpSlot();
    }
    // The pool takes its sets with it
    if (warpPool != VK_NULL_HANDLE) vk.DestroyDescriptorPool(device, warpPool, nullptr);
    if (warpPipeline != VK_NULL_HANDLE) vk.DestroyPipeline(device, warpPipeline, nullptr);
    if (warpUpscalePipeline != VK_NULL_HANDLE) vk.DestroyPipeline(device, warpUpscalePipeline, nullptr);
    if (warpLayout != VK_NULL_HANDLE) vk.DestroyPipelineLayout(device, warpLayout, nullptr);
    if (warpUpscaleLayout != VK_NULL_HANDLE) vk.DestroyPipelineLayout(device, warpUpscaleLayout, nullptr);
    if (warpSetLayout != VK_NULL_HANDLE) vk.DestroyDescriptorSetLayout(device, warpSetLayout, nullptr);
    if (warpSampler != VK_NULL_HANDLE) vk.DestroySampler(device, warpSampler, nullptr);
    warpPool = VK_NULL_HANDLE;
    warpPipeline = VK_NULL_HANDLE;
    warpUpscalePipeline = VK_NULL_HANDLE;
    warpLayout = VK_NULL_HANDLE;
    warpUpscaleLayout = VK_NULL_HANDLE;
    warpSetLayout = VK_NULL_HANDLE;
    warpSampler = VK_NULL_HANDLE;
}

// A fence rather than a semaphore: the CPU waits for the display to free an image, so
// the GPU queue never holds a draw that blocks the next decode behind it. The fence is

}  // namespace pyrowave_renderer

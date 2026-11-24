// SPDX-License-Identifier: GPL-3.0-only
// Adapted from joemossjr16/artemis-android-pyrowave 387d3a5c.
// Internal declaration of pyrowave_renderer::Renderer shared across translation units.
#pragma once

#include "pyrowave_diagnostics.h"

namespace pyrowave_renderer {

class Renderer {
public:
    PyroWaveRecords records;
    SubmissionDiagnostics submissionDiagnostics;

    explicit Renderer(CustomDriverLibrary *custom = nullptr) : vk(custom) {}
    ~Renderer() { destroy(); }

    bool create(ANativeWindow *nativeWindow, int streamWidth, int streamHeight, int frameRate,
                float displayRefreshRate, float topRefreshRate, int pacingMode,
                bool fullChroma, bool tenBitOutput, bool fullRangeOutput, bool stretchOutput,
                int upscale, float sharpness, bool preacquireSetting);
    int submit(const uint8_t *data, size_t length, int64_t ptsUs, int64_t hostPresentationTimeUs,
               int64_t vsyncNs, int64_t intervalNs);
    void pushChunks(const uint8_t *data, size_t length, bool first);

    void drain();
    bool enableFrameWarp(const FrameWarpApi *api);
    void noteFrameNumber(uint32_t frame) {
        warpNextFrame = int64_t(frame);
        warpNumbered = true;
    }
    const char *getPresentModeName() const {
        return presentModeName(presentMode.load(std::memory_order_relaxed));
    }
    // Real VRR: frames are shown at the PC's frame times. Set by create(), then constant.
    bool showsHostTimes() const { return framePacingMode == HOST_TIMES_MODE; }

    void pollRenderedFrames();
    void setHdrMode(bool enabled, const uint8_t *metadata);
    void setTransfer(bool hlgTransfer);
    void takeEvents(std::vector<jlong> &completed, std::vector<jlong> &released,
                    std::vector<jlong> &rendered);

    std::string driverDescription;
    std::atomic<PyroWaveUpscalingReason> upscalingReason{UPSCALING_DISABLED};

    // The managed decoder owns its command buffers. An outer timestamp pair
    // would include CPU submission gaps, so report this metric unavailable.
    static constexpr uint32_t lastGpuDecodeUs = 0;
    // Averages over the last stats interval, shown in the overlay: the decode thread's wait
    // for a free plane set, the present thread's wait for a swapchain image, and the share of
    // decoded frames a newer one replaced before they reached the screen.
    std::atomic<float> lastWaitFreeSlotMs{0};
    std::atomic<float> lastWaitImageMs{-1};
    std::atomic<float> lastReplacedPercent{-1};
    // With the PC's frame times, how long a frame was held after it was ready, on average;
    // -1 in every other mode, before the first interval, and while no frame carried a time.
    std::atomic<float> lastHostHoldMs{-1};
    // Early record push over the last stats interval: the share of frames whose records were
    // in the decoder by their last packet (-1 before any frame came with chunks), and the
    // frames whose chunks were not the frame and went whole.
    std::atomic<float> lastEarlyPercent{-1};
    std::atomic<uint32_t> lastEarlyFallbacks{0};
    std::atomic<float> lastRecordLossPercent{0.0f};
    bool displayTimingSupported = false;
    // Filled by completion/presentation workers, drained by nativePollRenderedFrames.
    std::mutex eventsMutex;
    std::vector<jlong> completedFrames;  // (ptsUs, completionNs), published before release
    std::vector<jlong> renderedFrames;   // (ptsUs, presentNs) pairs; 0 when the surface lost timing
    std::vector<jlong> releasedFrames;   // (ptsUs, releaseNs, shown) triples
    // JNI polling is serialized by PyroWaveDecoderRenderer. Keep both sides of the swap
    // alive so the presenter and poller reuse their buffers after the first two batches.
    std::vector<jlong> completedBatch;
    std::vector<jlong> renderedBatch;
    std::vector<jlong> releasedBatch;
    std::vector<jlong> packedEvents;
    size_t packedEventsOffset = 0;

    // Present thread only, like pollRenderedFrames(): reused so polling doesn't allocate.
    std::vector<VkPastPresentationTimingGOOGLE> pastTimings;

    PyroWavePresentPoll presentPoll;

private:
    bool check(VkResult result, const char *what);
    bool createInstanceAndSurface();
    bool createDevice();
    bool checkDecoderLimits();
    bool warmUpDecoder();
    bool createDecoder();
    bool createPlane(Plane &plane, uint32_t planeWidth, uint32_t planeHeight);
    bool createImage(Plane &plane, uint32_t planeWidth, uint32_t planeHeight, VkFormat format,
                     VkImageUsageFlags usage);
    VkFormat planeFormat() const { return tenBit ? VK_FORMAT_R16_UNORM : VK_FORMAT_R8_UNORM; }
    bool createPlanes();
    bool createSwapchain();
    bool createSwapchainResources();
    void destroySwapchainResources();
    PyroWaveExtent surfaceExtent(const VkSurfaceCapabilitiesKHR &caps) const;
    bool surfaceSizeChanged(uint64_t now);
    bool recreateSwapchain();
    VkShaderModule createShader(const uint32_t *code, size_t size);
    bool createPipeline();
    VkPipeline buildPipeline(const uint32_t *fragCode, size_t fragSize, VkPipelineLayout layout,
                             VkRenderPass pass, uint32_t colorAttachments = 1);
    bool createUpscaler();
    bool createUpscalerResources();
    bool createFrameResources();

    struct SdkPush {
        pyrowave_decoder decoder;
        bool operator()(const uint8_t *packet, size_t size) const {
            return pyrowave_decoder_push_packet(decoder, packet, size) == PYROWAVE_SUCCESS;
        }
    };

    SdkPush sdkPush() const { return {decoder}; }

    void countEarly(int result);
    bool pushFrame(const uint8_t *data, size_t length);
    VkPipelineStageFlags decodeWriteStage() const;
    VkAccessFlags decodeWriteAccess() const;
    void planeBarrier(VkCommandBuffer cmd, const Slot &slot, VkPipelineStageFlags srcStage,
                      VkAccessFlags srcAccess, VkPipelineStageFlags dstStage,
                      VkAccessFlags dstAccess, VkImageLayout oldLayout);
    int reserveDecodeSlot();
    bool waitForDecode(int index);
    bool signalDecodeFence(Slot &slot);
    bool decodeFrame(int index, int64_t ptsUs, int64_t hostPresentationTimeUs = 0);
    bool startWorkers();
    void stopWorkers();
    void completionLoop();
    void presentLoop();
    bool presentSlot(int index);
    bool recordDraw(int index, uint32_t imageIndex, const FrameWarpApi *warp, int32_t *warpMask);

    bool warpRedraws() const;
    bool warpWantsVsync() const;
    int64_t nextWarpVsync(int64_t now) const;

    // Frame Warp, present thread with slotMutex held (`lock`), while a picture is held for redraws:
    // waits as presentLoop() does for a new picture, but only until the next vsync. That vsync
    // when it came first, else 0 (a new picture, or the renderer stopping).
    template <class Woken>
    int64_t waitForPictureOrVsync(std::unique_lock<std::mutex> &lock, Woken woken) {
        const int64_t vsyncNs = nextWarpVsync(nowNs());
        while (!woken()) {
            const int64_t remainingNs = vsyncNs - nowNs();
            if (remainingNs <= 0) return vsyncNs;
            // Display times of shown frames arrive a little after their present; keep asking
            // for them meanwhile, as presentLoop() does
            const bool polling = !pendingPresents.empty();
            const int64_t pollNs = polling ? presentPoll.waitNs(nowNs(), driverRefreshPeriodNs) : remainingNs;
            const int64_t waitNs = std::min<int64_t>(remainingNs, pollNs);
            if (waitNs > 0 && slotReady.wait_for(lock, std::chrono::nanoseconds(waitNs), woken)) break;
            if (polling && pollNs <= waitNs) {
                lock.unlock();
                pollRenderedFrames();
                lock.lock();
            }
        }
        return 0;
    }

    int32_t setWarpShift(int index, const FrameWarpApi *warp, int32_t mask, float *out, uint32_t *seq);
    void warpShown(int index, const float *out, int32_t code, uint32_t seq, int64_t dueNs, bool newPicture);
    bool redrawSlot(int index, int64_t vsyncNs);
    int32_t recordWarpMask(int index, VkCommandBuffer cmd);
    bool replaceWarpMask(WarpSlot &slot, uint32_t width, uint32_t height);
    int32_t warpMemoryType(uint32_t bits, VkMemoryPropertyFlags flags);
    bool createMappedBuffer(VkDeviceSize size, VkBufferUsageFlags usage, VkBuffer *buffer,
                            VkDeviceMemory *memory, void **mapped);
    bool createWarpMask(WarpMask &mask, uint32_t width, uint32_t height);
    void destroyWarpMask(WarpMask &mask);
    void writeWarpDescriptors(const WarpSlot &slot);
    bool createWarpResources();
    void destroyWarpResources();

    PyroAcquire acquireImage(uint32_t *imageIndex, uint64_t timeoutNs);
    bool settleAcquireFence();
    void configurePreacquire(bool setting);
    bool createPresentClock(float topRefreshRate);
    void refreshDisplayClock();
    void dropSlot(int index);
    void pushRelease(int64_t ptsUs, int64_t releaseNs, bool shown);
    bool applyPendingHdr();
    void destroy();

    static void *dataspaceSymbol(const char *name);
    bool applyWindowDataspace();
    void checkHlgWindow();
    void logHdrOutput();
    void applyHdrMetadata();
    void logDecodeStatsIfDue(uint64_t now);
    void logEarlyStats(uint32_t done, uint32_t fallback, uint32_t whole);
    void logPresentStatsIfDue(uint64_t now);

    VulkanLoader vk;
    ANativeWindow *window = nullptr;
    uint32_t width = 0;
    uint32_t height = 0;
    bool chroma444 = false;
    bool tenBit = false;
    bool hdr = false;
    // HDR is HLG, else PQ; owned by the present thread like hdr.
    bool hlg = false;
    bool fullRange = false;
    std::atomic<bool> failed{false};
    VkHdrMetadataEXT hdrMetadata = {VK_STRUCTURE_TYPE_HDR_METADATA_EXT};
    std::vector<const char *> instanceExtensions, deviceExtensions;

    // Kept alive for PyroWave, which reads the create infos after device creation.
    VkApplicationInfo appInfo = {};
    VkInstanceCreateInfo instanceInfo = {};
    float queuePriorities[2] = {1.0f, 1.0f};
    uint32_t queueCount = 1;
    VkDeviceQueueCreateInfo queueInfo = {};
    VkPhysicalDeviceVulkan13Features features13 = {};
    VkPhysicalDevicePresentModeFifoLatestReadyFeaturesEXT fifoLatestReadyFeatures = {};
    VkPhysicalDeviceVulkan12Features features12 = {};
    VkPhysicalDeviceVulkan11Features features11 = {};
    // Below Vulkan 1.3 or 1.2, the extension structs that stand in for the core ones.
    VkPhysicalDeviceSubgroupSizeControlFeaturesEXT sizeControlFeatures = {};
    VkPhysicalDevice16BitStorageFeatures storage16Features = {};
    VkPhysicalDevice8BitStorageFeaturesKHR storage8Features = {};
    VkPhysicalDeviceShaderFloat16Int8FeaturesKHR float16Int8Features = {};
    VkPhysicalDeviceFeatures2 features2 = {};
    VkDeviceCreateInfo deviceInfo = {};
    VkDeviceQueueCreateInfo pyroQueueInfo = {};
    VkDeviceCreateInfo pyroDeviceInfo = {};

    VkInstance instance = VK_NULL_HANDLE;
    VkSurfaceKHR surface = VK_NULL_HANDLE;
    VkPhysicalDevice physicalDevice = VK_NULL_HANDLE;
    uint32_t queueFamily = 0;
    VkDevice device = VK_NULL_HANDLE;
    VkQueue queue = VK_NULL_HANDLE;
    VkQueue decodeQueue = VK_NULL_HANDLE;

    pyrowave_device pyroDevice = nullptr;
    pyrowave_decoder decoder = nullptr;
    // Plane sets decoded into in turn, so decoding never waits for a frame on its way to the screen.
    static constexpr int SLOT_COUNT = PyroWaveSchedule::SLOT_COUNT;
    Slot slots[SLOT_COUNT];
    PyroWaveSchedule schedule;
    PyroWavePresentTiming presentTiming;
    int framePacingMode = 0;
    // Real VRR: frames at the PC's frame times (PyroWavePresentTiming::Mode::HostTimes), from
    // this renderer's own shared present clock. Null in every other mode. Made before the
    // workers start, used by the present thread alone, freed after it ended.
    static constexpr int HOST_TIMES_MODE = int(PyroWavePresentTiming::Mode::HostTimes);
    RpPresentClock *presentClock = nullptr;
    std::mutex clockMutex;
    int64_t displayClockNs = 0;
    int64_t displayClockIntervalNs = 0;
    uint64_t lastRefreshCheckUs = 0;
    int64_t driverRefreshPeriodNs = 0;
    std::deque<int> pendingDecodes;
    std::condition_variable decodeReady;
    std::thread completer;

    // Guards the hand-over between the decode and present threads, and pending HDR changes.
    std::mutex slotMutex;
    std::condition_variable slotReady;
    uint32_t replacedSinceReport = 0;
    bool stopPresenter = false;
    bool hdrPending = false;
    bool pendingHdr = false;
    bool transferPending = false;
    bool pendingHlg = false;
    VkHdrMetadataEXT pendingHdrMetadata = {VK_STRUCTURE_TYPE_HDR_METADATA_EXT};
    std::thread presenter;
    std::mutex decodeQueueMutex;
    std::mutex renderQueueMutex;
    bool fragmentPath = false;
    PresentationDiagnostics presentationDiagnostics;
    PacingDiagnostics pacingDiagnostics;
    uint64_t lastSizeCheckUs = 0;

    VkFormat swapchainFormat = VK_FORMAT_UNDEFINED;
    VkColorSpaceKHR swapchainColorSpace = VK_COLOR_SPACE_SRGB_NONLINEAR_KHR;
    bool dataspaceOverride = false;
    // An HLG window dataspace was set and the next frame shown reads it back.
    bool hlgReadbackPending = false;
    // What the surface listed when the format was chosen; a transfer change keeps the format.
    std::vector<PyroSurfaceFormat> surfaceFormats;
    std::atomic<VkPresentModeKHR> presentMode{VK_PRESENT_MODE_FIFO_KHR};
    VkSwapchainKHR swapchain = VK_NULL_HANDLE;
    VkExtent2D swapchainExtent = {};
    PyroWaveExtent displayExtent = {};
    VkSurfaceTransformFlagBitsKHR surfaceTransform = VK_SURFACE_TRANSFORM_IDENTITY_BIT_KHR;
    int32_t rotationQuarterTurns = 0;
    std::vector<VkImage> swapchainImages;
    std::vector<VkImageView> swapchainViews;
    std::vector<VkFramebuffer> framebuffers;
    std::vector<VkSemaphore> renderDone;
    uint32_t nextPresentId = 0;
    struct PendingPresent {
        uint32_t id;
        int64_t ptsUs;
        uint64_t queuedUs;
        int64_t completedNs, releasedNs;
    };
    std::deque<PendingPresent> pendingPresents;

    VkRenderPass renderPass = VK_NULL_HANDLE;
    VkSampler sampler = VK_NULL_HANDLE;
    VkDescriptorSetLayout setLayout = VK_NULL_HANDLE;
    VkPipelineLayout pipelineLayout = VK_NULL_HANDLE;
    VkDescriptorPool descriptorPool = VK_NULL_HANDLE;
    VkPipeline pipeline = VK_NULL_HANDLE;

    int upscaleMode = UPSCALE_OFF;
    bool stretch = false;
    float edgeSharpness = 1.5f;
    VkPipelineLayout upscaleLayout = VK_NULL_HANDLE;
    VkPipeline upscalePipeline = VK_NULL_HANDLE;
    VkDeviceQueueGlobalPriorityCreateInfoEXT globalPriority = {};
    bool highPriorityQueue = false;

    VkCommandPool commandPool = VK_NULL_HANDLE;
    VkCommandPool drawCommandPool = VK_NULL_HANDLE;
    VkFence startupFence = VK_NULL_HANDLE;
    VkSemaphore decodeTimeline = VK_NULL_HANDLE;
    uint64_t lastDecodeValue = 0;
    // A device below Vulkan 1.3 has no timeline semaphores here. PyroWave then signals the
    // binary decodeSemaphore, and one wait on it right after each decode, on the shared queue,
    // signals the slot's decodeFence. That wait also orders the next decode and the draw.
    bool legacySync = false;
    VkSemaphore decodeSemaphore = VK_NULL_HANDLE;
    VkFence acquireFence = VK_NULL_HANDLE;
    // Present thread, or teardown after it ended: the image between acquire and present.
    PyroWaveImageHold imageHold;
    bool preacquireImages = false;

    // This command buffer only initializes plane layouts before workers start.
    // SDK-owned per-pass GPU timings are logged; completion times remain in CSV.
    static constexpr uint64_t STATS_INTERVAL_US = 5'000'000;
    VkCommandBuffer decodeCommandBuffer = VK_NULL_HANDLE;
    int frameRateHz = 60;
    float displayRefreshRateHz = 0;

    // Frame Warp (docs/frame-warp.md), made by enableFrameWarp() once the stream offers it. Until
    // then warpApi is null and the renderer draws, presents and waits exactly as without it: no
    // extra pass, descriptor set or redraw.
    std::atomic<const FrameWarpApi *> warpApi{nullptr};
    std::mutex warpSetupMutex;
    WarpFunctions warpVk;
    WarpSlot warpSlots[SLOT_COUNT];
    VkSampler warpSampler = VK_NULL_HANDLE;
    VkDescriptorSetLayout warpSetLayout = VK_NULL_HANDLE;
    VkDescriptorPool warpPool = VK_NULL_HANDLE;
    VkPipelineLayout warpLayout = VK_NULL_HANDLE;
    VkPipelineLayout warpUpscaleLayout = VK_NULL_HANDLE;
    VkPipeline warpPipeline = VK_NULL_HANDLE;
    VkPipeline warpUpscalePipeline = VK_NULL_HANDLE;
    // Present thread: the host's newest HUD mask; whether a mask image could not be made (the HUD
    // then shifts with the picture); what the picture on screen was drawn with (frame_warp_mask.h);
    // whether the vsync after a new picture is still to be looked at (warpWantsVsync()); when the
    // last present was queued; and the vsync a paced present asked for, before which nothing is
    // drawn again.
    FrameWarpCells warpCells;
    bool warpMaskFailed = false;
    FrameWarpDrawn warpDrawn;
    bool warpLookAfterPicture = false;
    int64_t warpLastPresentNs = 0;
    int64_t warpDueNs = 0;
    // Submitting thread (serialized by PyroWaveDecoderRenderer): the host's number for the frame
    // submitted next (noteFrameNumber()), and whether numbers come at all.
    int64_t warpNextFrame = -1;
    bool warpNumbered = false;

    // Each thread keeps its own counters.
    struct {
        uint64_t startUs = 0;
        uint32_t frames = 0;
        uint32_t earlyDone = 0, earlyFallback = 0, earlyWhole = 0;
    } decodeStats;
    struct {
        uint64_t startUs = 0;
        uint32_t frames = 0;
        uint32_t replaced = 0;
        uint64_t acquireWaitUs = 0;
        // PC frame times: how long the frames with a time were held after they were ready.
        uint32_t heldFrames = 0;
        uint64_t heldNs = 0;
    } presentStats;
};

}  // namespace pyrowave_renderer

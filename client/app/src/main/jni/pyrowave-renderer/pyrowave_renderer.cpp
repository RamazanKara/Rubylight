// SPDX-License-Identifier: GPL-3.0-only
// Adapted from joemossjr16/artemis-android-pyrowave 387d3a5c.
// Nightjar: packet framing, feature gates, HDR/10-bit output and completion timing.
// PyroWave video renderer for Moonlight Android.
//
// PyroWave is an intra-only wavelet codec decoded with Vulkan compute, so it cannot
// go through MediaCodec. This renderer owns a Vulkan device and a swapchain on the
// stream's Surface, lends the device to PyroWave, decodes each frame into three
// Y/Cb/Cr plane images and converts them to RGB in a fragment shader.
//
// Vulkan is loaded at runtime (libvulkan.so is not available on every Android
// version this app supports), so this library links neither libvulkan nor any
// Vulkan prototypes.


#include "pyrowave_renderer_internal.h"

static_assert(PYRO_DATASPACE_SRGB == ADATASPACE_SRGB && PYRO_DATASPACE_BT2020_PQ == ADATASPACE_BT2020_PQ &&
              PYRO_DATASPACE_BT2020_HLG == ADATASPACE_BT2020_HLG,
              "window dataspace constants in presentation.h must match the NDK");
static_assert(PYRO_FORMAT_A2B10G10R10 == VK_FORMAT_A2B10G10R10_UNORM_PACK32 &&
              PYRO_FORMAT_A2R10G10B10 == VK_FORMAT_A2R10G10B10_UNORM_PACK32 &&
              PYRO_FORMAT_R8G8B8A8 == VK_FORMAT_R8G8B8A8_UNORM && PYRO_FORMAT_B8G8R8A8 == VK_FORMAT_B8G8R8A8_UNORM &&
              PYRO_COLORSPACE_SRGB_NONLINEAR == VK_COLOR_SPACE_SRGB_NONLINEAR_KHR &&
              PYRO_COLORSPACE_HDR10_ST2084 == VK_COLOR_SPACE_HDR10_ST2084_EXT &&
              PYRO_COLORSPACE_HDR10_HLG == VK_COLOR_SPACE_HDR10_HLG_EXT,
              "surface format constants in presentation.h must match Vulkan");
static_assert(PYRO_VULKAN_1_1 == VK_API_VERSION_1_1 && PYRO_VULKAN_1_2 == VK_API_VERSION_1_2 &&
              PYRO_VULKAN_1_3 == VK_API_VERSION_1_3,
              "version constants in device_path.h must match Vulkan");

using namespace pyrowave_renderer;

extern "C" {

JNIEXPORT jint JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeGetHdrSurfaceFailure(JNIEnv *, jclass) {
    return hdrSurfaceFailure;
}

JNIEXPORT jint JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeGetReadiness(JNIEnv *env, jclass, jboolean tenBit,
                                                                            jstring hooks, jstring directory, jstring name) {
    CustomDriverLibrary *custom = directory == nullptr ? nullptr :
        customDriverLibrary(jniString(env, hooks), jniString(env, directory), jniString(env, name));
    if (directory != nullptr && custom == nullptr) return PROBE_FAILED;
    // Selection changes use a different immutable package and receive their own capability probe.
    static std::map<std::pair<CustomDriverLibrary *, bool>, Readiness> results;
    std::lock_guard<std::mutex> lock(readinessMutex);
    auto key = std::make_pair(custom, tenBit != 0);
    auto found = results.find(key);
    if (found == results.end()) found = results.emplace(key, probeReadiness(tenBit != 0, custom)).first;
    return found->second;
}

// Why the custom driver in directory did not load (the dynamic linker's message), or null when it
// loaded or was not tried. Read after nativeGetReadiness, whose probe opens the driver first.
JNIEXPORT jstring JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeGetCustomDriverError(JNIEnv *env, jclass, jstring hooks,
                                                                                    jstring directory, jstring name) {
    if (directory == nullptr) return nullptr;
    CustomDriverLibrary *custom = customDriverLibrary(jniString(env, hooks), jniString(env, directory), jniString(env, name));
    if (custom == nullptr) return nullptr;
    std::lock_guard<std::mutex> lock(readinessMutex);
    return custom->error[0] ? env->NewStringUTF(custom->error) : nullptr;
}

JNIEXPORT jlong JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeCreate(JNIEnv *env, jclass, jobject surface,
                                                                      jint width, jint height, jint frameRate, jfloat displayRefreshRate,
                                                                      jfloat topRefreshRate, jint pacingMode,
                                                                      jboolean chroma444, jboolean tenBit, jboolean fullRange, jboolean stretch,
                                                                      jint upscale, jfloat edgeSharpness, jboolean preacquire,
                                                                      jstring hooks, jstring directory, jstring name) {
    if (surface == nullptr || width < 64 || height < 64 || width > 16384 || height > 16384 || (!chroma444 && ((width & 1) || (height & 1)))) {
        LOGE("PyroWave needs a surface and positive dimensions, even for 4:2:0 (%dx%d)", width, height);
        return 0;
    }
    CustomDriverLibrary *custom = directory == nullptr ? nullptr :
        customDriverLibrary(jniString(env, hooks), jniString(env, directory), jniString(env, name));
    if (directory != nullptr && custom == nullptr) return 0;
    ANativeWindow *window = ANativeWindow_fromSurface(env, surface);
    if (window == nullptr) {
        return 0;
    }
    auto renderer = std::make_unique<Renderer>(custom);
    if (!renderer->create(window, width, height, frameRate, displayRefreshRate, topRefreshRate, pacingMode,
                          chroma444, tenBit, fullRange, stretch,
                          upscale, edgeSharpness, preacquire)) {
        return 0;  // The renderer releases the window.
    }
    LOGI("PyroWave renderer ready for %dx%d %s", width, height, chroma444 ? "4:4:4" : "4:2:0");
    return reinterpret_cast<jlong>(renderer.release());
}

static jlong packSubmitResult(int status, Renderer *renderer) {
    float loss = renderer != nullptr ? renderer->lastRecordLossPercent.load(std::memory_order_relaxed) : 0.0f;
    uint32_t lossBits = 0;
    std::memcpy(&lossBits, &loss, sizeof(lossBits));
    return (static_cast<jlong>(lossBits) << 32) | static_cast<uint32_t>(status);
}

JNIEXPORT jlong JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeSubmitFrame(JNIEnv *env, jclass, jlong handle,
                                                                           jbyteArray data, jint length, jlong ptsUs,
                                                                           jint frameNumber, jboolean frameWarp,
                                                                           jlong hostPresentationTimeUs,
                                                                           jlong vsyncNs, jlong intervalNs) {
    auto *renderer = reinterpret_cast<Renderer *>(handle);
    if (renderer == nullptr) return packSubmitResult(SUBMIT_ERROR, nullptr);
    SubmissionDiagnostics::Frame timing(renderer->submissionDiagnostics, length > 0 ? size_t(length) : 0);
    if (data == nullptr || length <= 0) return packSubmitResult(SUBMIT_ERROR, renderer);
    static thread_local std::vector<uint8_t> frame;
    {
        SubmissionDiagnostics::Stage copy(renderer->submissionDiagnostics, SubmissionDiagnostics::COPY);
        if (length > env->GetArrayLength(data)) return packSubmitResult(SUBMIT_ERROR, renderer);
        frame.resize(size_t(length));
        env->GetByteArrayRegion(data, 0, length, reinterpret_cast<jbyte *>(frame.data()));
        if (env->ExceptionCheck()) return packSubmitResult(SUBMIT_ERROR, renderer);
    }
    if (frameWarp) renderer->noteFrameNumber(uint32_t(frameNumber));
    timing.framing(length >= 4 && pyroReadLe32(frame.data()) == 0);
    timing.result = renderer->submit(frame.data(), frame.size(), ptsUs, hostPresentationTimeUs, vsyncNs, intervalNs);
    return packSubmitResult(timing.result, renderer);
}

// The Rust engine's frame in place (direct ByteBuffer fallback).
JNIEXPORT jlong JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeSubmitDirectFrame(JNIEnv *env, jclass, jlong handle,
                                                                                 jobject data, jint length, jlong ptsUs,
                                                                                 jint frameNumber, jboolean frameWarp,
                                                                                 jlong hostPresentationTimeUs,
                                                                                 jlong vsyncNs, jlong intervalNs) {
    auto *renderer = reinterpret_cast<Renderer *>(handle);
    if (renderer == nullptr) return packSubmitResult(SUBMIT_ERROR, nullptr);
    SubmissionDiagnostics::Frame timing(renderer->submissionDiagnostics, length > 0 ? size_t(length) : 0);
    if (data == nullptr || length <= 0) return packSubmitResult(SUBMIT_ERROR, renderer);
    const uint8_t *bytes;
    {
        SubmissionDiagnostics::Stage copy(renderer->submissionDiagnostics, SubmissionDiagnostics::COPY);
        bytes = static_cast<const uint8_t *>(env->GetDirectBufferAddress(data));
        if (bytes == nullptr || jlong(length) > env->GetDirectBufferCapacity(data)) {
            LOGE("PyroWave frame handed over in place without a direct buffer of %d bytes", length);
            return packSubmitResult(SUBMIT_ERROR, renderer);
        }
    }
    if (frameWarp) renderer->noteFrameNumber(uint32_t(frameNumber));
    timing.framing(length >= 4 && pyroReadLe32(bytes) == 0);
    timing.result = renderer->submit(bytes, size_t(length), ptsUs, hostPresentationTimeUs, vsyncNs, intervalNs);
    return packSubmitResult(timing.result, renderer);
}

// Raw native pointer fast path (zero NewDirectByteBuffer JNI upcall in Rust).
JNIEXPORT jlong JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeSubmitRawFrame(JNIEnv *, jclass, jlong handle,
                                                                              jlong dataPtr, jint length, jlong ptsUs,
                                                                              jint frameNumber, jboolean frameWarp,
                                                                              jlong hostPresentationTimeUs,
                                                                              jlong vsyncNs, jlong intervalNs) {
    auto *renderer = reinterpret_cast<Renderer *>(handle);
    if (renderer == nullptr) return packSubmitResult(SUBMIT_ERROR, nullptr);
    SubmissionDiagnostics::Frame timing(renderer->submissionDiagnostics, length > 0 ? size_t(length) : 0);
    const auto *bytes = reinterpret_cast<const uint8_t *>(static_cast<intptr_t>(dataPtr));
    if (bytes == nullptr || length <= 0) return packSubmitResult(SUBMIT_ERROR, renderer);
    if (frameWarp) renderer->noteFrameNumber(uint32_t(frameNumber));
    timing.framing(length >= 4 && pyroReadLe32(bytes) == 0);
    timing.result = renderer->submit(bytes, size_t(length), ptsUs, hostPresentationTimeUs, vsyncNs, intervalNs);
    return packSubmitResult(timing.result, renderer);
}

JNIEXPORT void JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeSubmitChunks(JNIEnv *env, jclass, jlong handle,
                                                                            jbyteArray data, jint length,
                                                                            jboolean first) {
    auto *renderer = reinterpret_cast<Renderer *>(handle);
    if (renderer == nullptr || data == nullptr || length <= 0) return;
    SubmissionDiagnostics::Chunks timing(renderer->submissionDiagnostics);
    static thread_local std::vector<uint8_t> chunks;
    if (length > env->GetArrayLength(data)) return;
    chunks.resize(size_t(length));
    env->GetByteArrayRegion(data, 0, length, reinterpret_cast<jbyte *>(chunks.data()));
    if (env->ExceptionCheck()) return;
    renderer->pushChunks(chunks.data(), chunks.size(), first == JNI_TRUE);
}

JNIEXPORT void JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeSubmitChunksDirect(JNIEnv *env, jclass, jlong handle,
                                                                                  jobject data, jint length,
                                                                                  jboolean first) {
    auto *renderer = reinterpret_cast<Renderer *>(handle);
    if (renderer == nullptr || data == nullptr || length <= 0) return;
    SubmissionDiagnostics::Chunks timing(renderer->submissionDiagnostics);
    const auto *bytes = static_cast<const uint8_t *>(env->GetDirectBufferAddress(data));
    if (bytes == nullptr || jlong(length) > env->GetDirectBufferCapacity(data)) {
        LOGE("PyroWave chunks handed over in place without a direct buffer of %d bytes", length);
        return;
    }
    renderer->pushChunks(bytes, size_t(length), first == JNI_TRUE);
}

// Raw native pointer fast path for early chunks (zero NewDirectByteBuffer JNI upcall in Rust).
JNIEXPORT void JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeSubmitChunksRaw(JNIEnv *, jclass, jlong handle,
                                                                               jlong dataPtr, jint length,
                                                                               jboolean first) {
    auto *renderer = reinterpret_cast<Renderer *>(handle);
    const auto *bytes = reinterpret_cast<const uint8_t *>(static_cast<intptr_t>(dataPtr));
    if (renderer == nullptr || bytes == nullptr || length <= 0) return;
    SubmissionDiagnostics::Chunks timing(renderer->submissionDiagnostics);
    renderer->pushChunks(bytes, size_t(length), first == JNI_TRUE);
}

// debug.nightjar.pyro_early=0 disables early record push for the next stream.
JNIEXPORT jboolean JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeEarlyRecordsAllowed(JNIEnv *, jclass) {
    char value[PROP_VALUE_MAX] = {};
    __system_property_get("debug.nightjar.pyro_early", value);
    return std::strcmp(value, "0") == 0 ? JNI_FALSE : JNI_TRUE;
}

// Batches decoded/released/rendered events into a reused Java long[] (stride 4: kind, ptsUs, timeNs, flags)
// to eliminate per-event JNI upcalls. Returns eventCount (>0 done, <0 more remain, 0 empty).
JNIEXPORT jint JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativePollRenderedFrames(JNIEnv *env, jclass, jlong handle,
                                                                                 jlongArray out) {
    auto *renderer = reinterpret_cast<Renderer *>(handle);
    if (renderer == nullptr || out == nullptr) return 0;
    const jsize outLen = env->GetArrayLength(out);
    const size_t maxLongs = outLen > 0 ? (size_t(outLen) & ~size_t(3)) : 0;
    if (maxLongs == 0) return 0;
    auto &packed = renderer->packedEvents;
    if (renderer->packedEventsOffset >= packed.size()) {
        packed.clear();
        renderer->packedEventsOffset = 0;
        const jlong timingBit = renderer->displayTimingSupported ? 2 : 0;
        auto &completed = renderer->completedBatch;
        auto &released = renderer->releasedBatch;
        auto &rendered = renderer->renderedBatch;
        renderer->takeEvents(completed, released, rendered);
        if (completed.empty() && released.empty() && rendered.empty()) return 0;
        packed.reserve((completed.size() / 2 + released.size() / 3 + rendered.size() / 2) * 4);
        for (size_t i = 0; i + 2 <= completed.size(); i += 2) {
            packed.push_back(0); packed.push_back(completed[i]); packed.push_back(completed[i + 1]); packed.push_back(0);
        }
        for (size_t i = 0; i + 3 <= released.size(); i += 3) {
            packed.push_back(1); packed.push_back(released[i]); packed.push_back(released[i + 1]);
            packed.push_back((released[i + 2] ? 1 : 0) | timingBit);
        }
        for (size_t i = 0; i + 2 <= rendered.size(); i += 2) {
            packed.push_back(2); packed.push_back(rendered[i]); packed.push_back(rendered[i + 1]); packed.push_back(0);
        }
    }
    const size_t remaining = packed.size() - renderer->packedEventsOffset;
    const size_t take = remaining < maxLongs ? remaining : maxLongs;
    env->SetLongArrayRegion(out, 0, jsize(take), packed.data() + renderer->packedEventsOffset);
    renderer->packedEventsOffset += take;
    const bool more = renderer->packedEventsOffset < packed.size();
    if (!more) {
        packed.clear();
        renderer->packedEventsOffset = 0;
    }
    const jint count = jint(take / 4);
    return more ? -count : count;
}

JNIEXPORT jint JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeGetLastGpuDecodeUs(JNIEnv *, jclass, jlong handle) {
    auto *renderer = reinterpret_cast<Renderer *>(handle);
    return renderer != nullptr ? jint(renderer->lastGpuDecodeUs) : 0;
}

JNIEXPORT jfloatArray JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeGetEarlyRecords(JNIEnv *env, jclass, jlong handle) {
    auto *renderer = reinterpret_cast<Renderer *>(handle);
    if (renderer == nullptr) return nullptr;
    jfloat values[2] = {renderer->lastEarlyPercent, float(renderer->lastEarlyFallbacks.load())};
    jfloatArray result = env->NewFloatArray(2);
    if (result != nullptr) env->SetFloatArrayRegion(result, 0, 2, values);
    return result;
}

JNIEXPORT jfloatArray JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeGetWaits(JNIEnv *env, jclass, jlong handle) {
    auto *renderer = reinterpret_cast<Renderer *>(handle);
    if (renderer == nullptr) return nullptr;
    jfloat values[4] = {renderer->lastWaitFreeSlotMs, renderer->lastWaitImageMs, renderer->lastReplacedPercent,
                        renderer->lastHostHoldMs};
    jfloatArray result = env->NewFloatArray(4);
    if (result != nullptr) env->SetFloatArrayRegion(result, 0, 4, values);
    return result;
}

JNIEXPORT jstring JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeGetPresentMode(JNIEnv *env, jclass, jlong handle) {
    auto *renderer = reinterpret_cast<Renderer *>(handle);
    if (renderer == nullptr) return env->NewStringUTF("unavailable");
    std::string mode = renderer->getPresentModeName();
    // Real VRR, in the words the desktop overlay's Pacing line has for it.
    if (renderer->showsHostTimes()) mode += ", PC frame times";
    return env->NewStringUTF(mode.c_str());
}

JNIEXPORT jstring JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeGetDriver(JNIEnv *env, jclass, jlong handle) {
    auto *renderer = reinterpret_cast<Renderer *>(handle);
    return env->NewStringUTF(renderer != nullptr ? renderer->driverDescription.c_str() : "");
}

JNIEXPORT jint JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeGetUpscalingReason(JNIEnv *, jclass, jlong handle) {
    auto *renderer = reinterpret_cast<Renderer *>(handle);
    return renderer != nullptr ? renderer->upscalingReason.load() : UPSCALING_DISABLED;
}

JNIEXPORT jboolean JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeSetGpuMaxClocks(JNIEnv *, jclass, jboolean enabled) {
    return setAdrenoMaxClocks(enabled);
}

JNIEXPORT void JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeSetHdrMode(JNIEnv *env, jclass, jlong handle,
                                                                       jboolean enabled, jbyteArray metadata) {
    auto *renderer = reinterpret_cast<Renderer *>(handle);
    if (renderer == nullptr) return;
    uint8_t bytes[24];
    bool valid = metadata && env->GetArrayLength(metadata) >= 24;
    if (valid) {
        env->GetByteArrayRegion(metadata, 0, 24, reinterpret_cast<jbyte *>(bytes));
        if (env->ExceptionCheck()) return;
    }
    renderer->setHdrMode(enabled, valid ? bytes : nullptr);
}

JNIEXPORT void JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeSetTransfer(JNIEnv *, jclass, jlong handle,
                                                                       jboolean hlg) {
    auto *renderer = reinterpret_cast<Renderer *>(handle);
    if (renderer == nullptr) return;
    renderer->setTransfer(hlg);
}

// Frame Warp for this stream (docs/frame-warp.md): `api` is MoonBridge.frameWarpApi(), the table in
// libnightjar_core.so. False when this build does not know the table or the renderer cannot draw it.
JNIEXPORT jboolean JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeSetFrameWarp(JNIEnv *, jclass, jlong handle,
                                                                          jlong api) {
    auto *renderer = reinterpret_cast<Renderer *>(handle);
    const FrameWarpApi *table = frameWarpApiFrom(api);
    return renderer != nullptr && table != nullptr && renderer->enableFrameWarp(table) ? JNI_TRUE : JNI_FALSE;
}

JNIEXPORT void JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeDrain(JNIEnv *, jclass, jlong handle) {
    auto *renderer = reinterpret_cast<Renderer *>(handle);
    if (renderer != nullptr) renderer->drain();
}

JNIEXPORT void JNICALL
Java_com_limelight_binding_video_PyroWaveDecoderRenderer_nativeDestroy(JNIEnv *, jclass, jlong handle) {
    delete reinterpret_cast<Renderer *>(handle);
}

}  // extern "C"

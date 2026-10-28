#include <jni.h>
#include <android/native_window_jni.h>
#include <vector>

#include "output_format.h"
#include "vulkan_renderer.h"

using vkr::RendererConfig;
using vkr::VulkanRenderer;

namespace {
    VulkanRenderer* fromHandle(jlong handle) {
        return reinterpret_cast<VulkanRenderer*>(handle);
    }
}

extern "C" JNIEXPORT jboolean JNICALL
Java_com_limelight_binding_video_VulkanRendererBridge_nativeProbe(JNIEnv*, jclass) {
    return VulkanRenderer::probe() ? JNI_TRUE : JNI_FALSE;
}

extern "C" JNIEXPORT jboolean JNICALL
Java_com_limelight_binding_video_VulkanRendererBridge_nativeProbedHdrColorSpaces(JNIEnv*, jclass) {
    return VulkanRenderer::probedHdrColorSpaces() ? JNI_TRUE : JNI_FALSE;
}

extern "C" JNIEXPORT jstring JNICALL
Java_com_limelight_binding_video_VulkanRendererBridge_nativeLastError(JNIEnv* env, jclass) {
    return env->NewStringUTF(VulkanRenderer::lastError().c_str());
}

extern "C" JNIEXPORT jstring JNICALL
Java_com_limelight_binding_video_VulkanRendererBridge_nativeFatalReason(JNIEnv* env, jclass, jlong handle) {
    VulkanRenderer* renderer = fromHandle(handle);
    if (!renderer || !renderer->fatal()) {
        return nullptr;
    }
    return env->NewStringUTF(renderer->fatalReason().c_str());
}

extern "C" JNIEXPORT jlong JNICALL
Java_com_limelight_binding_video_VulkanRendererBridge_nativeCreate(
        JNIEnv* env, jclass, jobject outputSurface, jint streamWidth, jint streamHeight, jint streamFps,
        jint framePacing, jint jitterBuffer, jint ditherMode, jint colorspace, jboolean fullRange, jboolean tenBit,
        jboolean hlg, jfloat displayRefreshHz, jstring codec, jstring decoderName, jstring traceDirectory,
        jboolean frontBuffer) {
    ANativeWindow* output = ANativeWindow_fromSurface(env, outputSurface);
    if (!output) {
        return 0;
    }

    RendererConfig config;
    config.streamWidth = streamWidth;
    config.streamHeight = streamHeight;
    config.streamFps = streamFps;
    config.framePacing = framePacing;
    config.jitterBuffer = jitterBuffer;
    config.ditherMode = ditherMode;
    config.colorspace = colorspace;
    config.fullRange = fullRange;
    config.tenBit = tenBit;
    config.hdrTransfer = hlg ? vkr::kHdrTransferHlg : vkr::kHdrTransferPq;
    config.displayRefreshHz = displayRefreshHz;
    config.frontBuffer = frontBuffer;
    auto toString = [env](jstring string) {
        std::string value;
        if (string) {
            const char* chars = env->GetStringUTFChars(string, nullptr);
            value = chars;
            env->ReleaseStringUTFChars(string, chars);
        }
        return value;
    };
    config.codec = toString(codec);
    config.decoderName = toString(decoderName);
    config.traceDirectory = toString(traceDirectory);

    std::unique_ptr<VulkanRenderer> renderer = VulkanRenderer::create(output, config);

    // The renderer holds its own reference
    ANativeWindow_release(output);
    return reinterpret_cast<jlong>(renderer.release());
}

extern "C" JNIEXPORT jobject JNICALL
Java_com_limelight_binding_video_VulkanRendererBridge_nativeGetDecoderSurface(JNIEnv* env, jclass, jlong handle) {
    const NdkApi* ndk = loadNdkApi();
    VulkanRenderer* renderer = fromHandle(handle);
    if (!ndk || !renderer || !renderer->decoderWindow()) {
        return nullptr;
    }
    return ndk->ANativeWindow_toSurface(env, renderer->decoderWindow());
}

extern "C" JNIEXPORT void JNICALL
Java_com_limelight_binding_video_VulkanRendererBridge_nativeSetHdrMode(
        JNIEnv* env, jclass, jlong handle, jboolean enabled, jbyteArray metadata) {
    VulkanRenderer* renderer = fromHandle(handle);
    if (!renderer) {
        return;
    }

    if (metadata) {
        jsize length = env->GetArrayLength(metadata);
        jbyte* bytes = env->GetByteArrayElements(metadata, nullptr);
        renderer->setHdrMode(enabled, reinterpret_cast<const uint8_t*>(bytes), static_cast<size_t>(length));
        env->ReleaseByteArrayElements(metadata, bytes, JNI_ABORT);
    }
    else {
        renderer->setHdrMode(enabled, nullptr, 0);
    }
}

extern "C" JNIEXPORT void JNICALL
Java_com_limelight_binding_video_VulkanRendererBridge_nativeSetJitterBuffer(JNIEnv*, jclass, jlong handle, jint jitterBuffer) {
    VulkanRenderer* renderer = fromHandle(handle);
    if (renderer) {
        renderer->setJitterBuffer(static_cast<vkr::JitterBuffer>(jitterBuffer));
    }
}

extern "C" JNIEXPORT void JNICALL
Java_com_limelight_binding_video_VulkanRendererBridge_nativeSetVsyncOffset(JNIEnv*, jclass, jlong handle,
                                                                          jlong offsetNs) {
    VulkanRenderer* renderer = fromHandle(handle);
    if (renderer) {
        renderer->setVsyncOffsetNs(offsetNs);
    }
}

extern "C" JNIEXPORT void JNICALL
Java_com_limelight_binding_video_VulkanRendererBridge_nativeNoteReceived(
        JNIEnv*, jclass, jlong handle, jlong hostPtsUs, jlong receiveUs, jlong enqueueUs, jchar hostProcessingLatency) {
    VulkanRenderer* renderer = fromHandle(handle);
    if (renderer) {
        renderer->noteReceived(hostPtsUs * 1000, receiveUs * 1000, enqueueUs * 1000,
                               static_cast<int64_t>(hostProcessingLatency) * 100'000);
    }
}

extern "C" JNIEXPORT void JNICALL
Java_com_limelight_binding_video_VulkanRendererBridge_nativeNoteHdr10Plus(
        JNIEnv* env, jclass, jlong handle, jlong hostPtsUs, jbyteArray t35) {
    VulkanRenderer* renderer = fromHandle(handle);
    if (!renderer) {
        return;
    }
    // Called only when the metadata changes
    std::vector<uint8_t> bytes(t35 ? static_cast<size_t>(env->GetArrayLength(t35)) : 0);
    if (!bytes.empty()) {
        env->GetByteArrayRegion(t35, 0, static_cast<jsize>(bytes.size()), reinterpret_cast<jbyte*>(bytes.data()));
    }
    renderer->noteHdr10Plus(hostPtsUs * 1000, bytes.data(), bytes.size());
}

extern "C" JNIEXPORT jint JNICALL
Java_com_limelight_binding_video_VulkanRendererBridge_nativeHdr10PlusState(JNIEnv*, jclass, jlong handle) {
    VulkanRenderer* renderer = fromHandle(handle);
    return renderer ? renderer->hdr10PlusState() : VulkanRenderer::kHdr10PlusNone;
}

extern "C" JNIEXPORT jint JNICALL
Java_com_limelight_binding_video_VulkanRendererBridge_nativeTakePresentedFrames(JNIEnv*, jclass, jlong handle) {
    VulkanRenderer* renderer = fromHandle(handle);
    return renderer ? static_cast<jint>(renderer->takePresentedFrames()) : 0;
}

extern "C" JNIEXPORT jboolean JNICALL
Java_com_limelight_binding_video_VulkanRendererBridge_nativeIsFrontBufferUnavailable(JNIEnv*, jclass, jlong handle) {
    VulkanRenderer* renderer = fromHandle(handle);
    return renderer && renderer->frontBufferUnavailable() ? JNI_TRUE : JNI_FALSE;
}

extern "C" JNIEXPORT jboolean JNICALL
Java_com_limelight_binding_video_VulkanRendererBridge_nativeIsFrontBufferActive(JNIEnv*, jclass, jlong handle) {
    VulkanRenderer* renderer = fromHandle(handle);
    return renderer && renderer->frontBufferActive() ? JNI_TRUE : JNI_FALSE;
}

extern "C" JNIEXPORT jboolean JNICALL
Java_com_limelight_binding_video_VulkanRendererBridge_nativeIsHdrToneMapped(JNIEnv*, jclass, jlong handle) {
    VulkanRenderer* renderer = fromHandle(handle);
    return renderer && renderer->hdrToneMapped() ? JNI_TRUE : JNI_FALSE;
}

extern "C" JNIEXPORT void JNICALL
Java_com_limelight_binding_video_VulkanRendererBridge_nativeTakeCaptureLatency(JNIEnv* env, jclass, jlong handle,
                                                                              jlongArray out) {
    VulkanRenderer* renderer = fromHandle(handle);
    if (!renderer) {
        return;
    }
    const VulkanRenderer::CaptureLatency latency = renderer->takeCaptureLatency();
    const jlong values[6] = {
            latency.decoded.totalNs, latency.decoded.frames, latency.decoded.unsynced,
            latency.presented.totalNs, latency.presented.frames, latency.presented.unsynced,
    };
    env->SetLongArrayRegion(out, 0, 6, values);
}

extern "C" JNIEXPORT jboolean JNICALL
Java_com_limelight_binding_video_VulkanRendererBridge_nativeReportsPresentTimes(JNIEnv*, jclass, jlong handle) {
    VulkanRenderer* renderer = fromHandle(handle);
    return renderer && renderer->reportsPresentTimes() ? JNI_TRUE : JNI_FALSE;
}

extern "C" JNIEXPORT jint JNICALL
Java_com_limelight_binding_video_VulkanRendererBridge_nativeTakePresentReports(JNIEnv* env, jclass, jlong handle,
                                                                              jlongArray out) {
    VulkanRenderer* renderer = fromHandle(handle);
    if (!renderer || !out) {
        return 0;
    }
    const jsize capacity = env->GetArrayLength(out) / 3;
    std::vector<int64_t> reports(static_cast<size_t>(capacity) * 3);
    const size_t n = renderer->takePresentReports(reports.data(), static_cast<size_t>(capacity));
    if (n > 0) {
        env->SetLongArrayRegion(out, 0, static_cast<jsize>(n * 3), reinterpret_cast<const jlong*>(reports.data()));
    }
    return static_cast<jint>(n);
}

extern "C" JNIEXPORT jstring JNICALL
Java_com_limelight_binding_video_VulkanRendererBridge_nativeGetRendererText(JNIEnv* env, jclass, jlong handle) {
    VulkanRenderer* renderer = fromHandle(handle);
    return renderer ? env->NewStringUTF(renderer->rendererText().c_str()) : nullptr;
}

extern "C" JNIEXPORT jstring JNICALL
Java_com_limelight_binding_video_VulkanRendererBridge_nativeGetPacingText(JNIEnv* env, jclass, jlong handle) {
    VulkanRenderer* renderer = fromHandle(handle);
    return renderer ? env->NewStringUTF(renderer->pacingText().c_str()) : nullptr;
}

extern "C" JNIEXPORT void JNICALL
Java_com_limelight_binding_video_VulkanRendererBridge_nativeStop(JNIEnv*, jclass, jlong handle) {
    VulkanRenderer* renderer = fromHandle(handle);
    if (renderer) {
        renderer->stop();
    }
}

extern "C" JNIEXPORT void JNICALL
Java_com_limelight_binding_video_VulkanRendererBridge_nativeDestroy(JNIEnv*, jclass, jlong handle) {
    delete fromHandle(handle);
}

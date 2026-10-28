package com.limelight.binding.video;

import android.view.Surface;

import com.limelight.LimeLog;

/**
 * Java side of the native Vulkan video renderer (app/src/main/jni/vulkan).
 *
 * The decoder renders into {@link #getDecoderSurface()}. Native code imports each decoded
 * frame into Vulkan without a copy, converts and dithers it, and presents it on the output
 * surface at the vsync its frame pacing mode picks.
 */
public class VulkanRendererBridge {
    /** A present report's screen time when the screen could not tell (a front buffer, say). */
    public static final long NO_SCANOUT = -1;

    private static Boolean supported;
    private static boolean hdrColorSpaces;
    private static String unsupportedReason;

    private long handle;
    private Surface decoderSurface;

    private VulkanRendererBridge(long handle) {
        this.handle = handle;
    }

    /**
     * Whether this device can run the renderer. Needs Android 10 for the NDK APIs it uses,
     * plus Vulkan 1.1 with AHardwareBuffer import and YCbCr sampling.
     */
    public static synchronized boolean isSupported() {
        if (supported == null) {
            long startNs = System.nanoTime();
            boolean result = false;
            try {
                System.loadLibrary("vulkan_renderer");
                result = nativeProbe();
                hdrColorSpaces = result && nativeProbedHdrColorSpaces();
                if (!result) {
                    unsupportedReason = nativeLastError();
                }
            } catch (UnsatisfiedLinkError e) {
                unsupportedReason = "native library unavailable: " + e.getMessage();
            }
            LimeLog.info("Vulkan renderer supported: " + result + " (probe took " +
                    (System.nanoTime() - startNs) / 1000000 + " ms)" +
                    (result ? ", HDR colour spaces: " + hdrColorSpaces : "") +
                    (result || unsupportedReason == null ? "" : ", " + unsupportedReason));
            supported = result;
        }
        return supported;
    }

    /**
     * Whether the renderer runs here and its instance has VK_EXT_swapchain_colorspace: it then shows
     * HLG as well as HDR10, through the HDR colour spaces (Android 15+) or the window dataspace.
     */
    public static synchronized boolean drawsHlg() {
        return isSupported() && hdrColorSpaces;
    }

    /** Why {@link #isSupported()} is false, or null if it is true or not yet known. */
    public static synchronized String getUnsupportedReason() {
        return unsupportedReason;
    }

    /** The native layer's last error text, or null; for the reason a start failed. */
    public static synchronized String getLastError() {
        return supported == null || !supported ? null : nativeLastError();
    }

    /**
     * Starts a renderer on the output surface, or returns null if it can't run there.
     *
     * @param framePacing one of PreferenceConfiguration.FRAME_PACING_*
     * @param jitterBuffer one of PreferenceConfiguration.JITTER_BUFFER_*
     * @param ditherMode 0 = off, 1 = low, 2 = high
     * @param colorspace one of MoonBridge.COLORSPACE_*
     * @param codec h264, hevc or av1, and decoderName the MediaCodec decoder, both recorded in
     *              pacer traces
     * @param traceDirectory where frame pacing traces are written when switched on with
     *                       {@code adb shell setprop debug.moonlight.pacer_trace 1}, or null
     * @param hlg the stream's HDR is HLG rather than PQ
     * @param frontBuffer draw into a front buffer the display reads directly, timed to each scan,
     *                    where the device has one
     */
    public static VulkanRendererBridge create(Surface output, int streamWidth, int streamHeight, int streamFps,
                                              int framePacing, int jitterBuffer, int ditherMode, int colorspace,
                                              boolean fullRange, boolean tenBit, boolean hlg, float displayRefreshHz,
                                              String codec, String decoderName, String traceDirectory,
                                              boolean frontBuffer) {
        if (!isSupported()) {
            return null;
        }

        long handle = nativeCreate(output, streamWidth, streamHeight, streamFps, framePacing, jitterBuffer, ditherMode,
                colorspace, fullRange, tenBit, hlg, displayRefreshHz, codec, decoderName, traceDirectory, frontBuffer);
        if (handle == 0) {
            LimeLog.warning("Vulkan renderer could not start: " + nativeLastError());
            return null;
        }

        VulkanRendererBridge bridge = new VulkanRendererBridge(handle);
        bridge.decoderSurface = nativeGetDecoderSurface(handle);
        if (bridge.decoderSurface == null) {
            LimeLog.warning("Vulkan renderer has no decoder surface: " + nativeLastError());
            bridge.destroy();
            return null;
        }
        return bridge;
    }

    /** Surface for the decoder to render into */
    public Surface getDecoderSurface() {
        return decoderSurface;
    }

    /**
     * Notes a frame's network timing as it's handed to the decoder, for the capture latency
     * statistics and the frame pacing trace, if one is being recorded. hostPtsUs is the
     * timestamp the frame is queued to the decoder with, which comes back on the decoded image.
     * All times in microseconds; receive and enqueue times are moonlight-common-c's. hostProcessingLatency is
     * the host's, in tenths of a millisecond (0 if the host doesn't report it).
     */
    public synchronized void noteFrameReceived(long hostPtsUs, long receiveTimeUs, long enqueueTimeUs, char hostProcessingLatency) {
        if (handle != 0) {
            nativeNoteReceived(handle, hostPtsUs, receiveTimeUs, enqueueTimeUs, hostProcessingLatency);
        }
    }

    /**
     * Switches the jitter buffer for the rest of the stream.
     * @param jitterBuffer one of PreferenceConfiguration.JITTER_BUFFER_*
     */
    public synchronized void setJitterBuffer(int jitterBuffer) {
        if (handle != 0) {
            nativeSetJitterBuffer(handle, jitterBuffer);
        }
    }

    /**
     * How long after the display's vsync Choreographer's frame times are
     * ({@link android.view.Display#getAppVsyncOffsetNanos()}), which changes with the display mode
     */
    public synchronized void setVsyncOffset(long offsetNs) {
        if (handle != 0) {
            nativeSetVsyncOffset(handle, offsetNs);
        }
    }

    public synchronized void setHdrMode(boolean enabled, byte[] hdrMetadata) {
        if (handle != 0) {
            nativeSetHdrMode(handle, enabled, hdrMetadata);
        }
    }

    /** Frames presented since the last call */
    public synchronized int takePresentedFrames() {
        return handle != 0 ? nativeTakePresentedFrames(handle) : 0;
    }

    /**
     * Whether a front buffer was asked for but the device couldn't give one, so frames go
     * through the compositor's queue as usual. Known once the first frame is drawn.
     */
    public synchronized boolean isFrontBufferUnavailable() {
        return handle != 0 && nativeIsFrontBufferUnavailable(handle);
    }

    /** Whether frames are drawn into a front buffer. Known once the first frame is drawn. */
    public synchronized boolean isFrontBufferActive() {
        return handle != 0 && nativeIsFrontBufferActive(handle);
    }

    /**
     * Whether the PC sends HDR but the surface took neither an HDR format nor the HDR window
     * dataspace, so the picture is tone mapped to SDR. Known once the HDR swapchain is made.
     */
    public synchronized boolean isHdrToneMapped() {
        return handle != 0 && nativeIsHdrToneMapped(handle);
    }

    /**
     * Capture latency since the last call, into out: the total time in nanoseconds from frames'
     * capture until they were decoded, how many frames that counts and how many of them were
     * measured without clock sync (and so without the network transit), then the same three
     * for until they reached the screen.
     */
    public synchronized void takeCaptureLatency(long[] out) {
        if (handle != 0) {
            nativeTakeCaptureLatency(handle, out);
        }
    }

    /**
     * Whether the screen reports when frames are shown, so {@link #takePresentReports(long[])} has
     * something to say. Without it there is no honest time to the screen.
     */
    public synchronized boolean reportsPresentTimes() {
        return handle != 0 && nativeReportsPresentTimes(handle);
    }

    /**
     * How frames ended up since the last call. out gets triples of the timestamp a frame was queued
     * to the decoder with (microseconds), the time vkQueuePresentKHR was called for it and the time
     * it reached the screen (both on the System.nanoTime() clock); the result is the number of
     * triples. A screen time of 0 means the frame never reached the screen (the queue time is 0
     * too), and {@link #NO_SCANOUT} that the screen could not tell.
     */
    public synchronized int takePresentReports(long[] out) {
        if (handle == 0) return 0;
        int n = nativeTakePresentReports(handle, out);
        for (int i = 0; i < n; i++) {
            out[3 * i] /= 1000;
        }
        return n;
    }

    /**
     * Why the renderer gave up (device lost, or submits, imports or the swapchain kept failing),
     * or null while it is healthy. Once set, no further frames are presented.
     */
    public synchronized String getFatalReason() {
        return handle != 0 ? nativeFatalReason(handle) : null;
    }

    /** One line for the performance overlay */
    public synchronized String getRendererText() {
        return handle != 0 ? nativeGetRendererText(handle) : null;
    }

    /**
     * Pacing numbers for the performance overlay: a headline and details, separated by a
     * newline. Either may be empty.
     */
    public synchronized String getPacingText() {
        return handle != 0 ? nativeGetPacingText(handle) : null;
    }

    /**
     * Stops presenting and lets go of the output surface. Must be called before the output
     * surface is destroyed.
     */
    public synchronized void stop() {
        if (handle != 0) {
            nativeStop(handle);
        }
    }

    /** Frees the renderer. The decoder must be released first. */
    public synchronized void destroy() {
        if (decoderSurface != null) {
            decoderSurface.release();
            decoderSurface = null;
        }
        if (handle != 0) {
            nativeDestroy(handle);
            handle = 0;
        }
    }

    private static native boolean nativeProbe();
    private static native boolean nativeProbedHdrColorSpaces();
    private static native String nativeLastError();
    private static native String nativeFatalReason(long handle);
    private static native long nativeCreate(Surface output, int streamWidth, int streamHeight, int streamFps,
                                            int framePacing, int jitterBuffer, int ditherMode, int colorspace,
                                            boolean fullRange, boolean tenBit, boolean hlg, float displayRefreshHz,
                                            String codec, String decoderName, String traceDirectory,
                                            boolean frontBuffer);
    private static native Surface nativeGetDecoderSurface(long handle);
    private static native void nativeNoteReceived(long handle, long hostPtsUs, long receiveTimeUs, long enqueueTimeUs,
                                                  char hostProcessingLatency);
    private static native void nativeSetHdrMode(long handle, boolean enabled, byte[] hdrMetadata);
    private static native void nativeSetJitterBuffer(long handle, int jitterBuffer);
    private static native void nativeSetVsyncOffset(long handle, long offsetNs);
    private static native int nativeTakePresentedFrames(long handle);
    private static native boolean nativeIsFrontBufferUnavailable(long handle);
    private static native boolean nativeIsFrontBufferActive(long handle);
    private static native boolean nativeIsHdrToneMapped(long handle);
    private static native void nativeTakeCaptureLatency(long handle, long[] out);
    private static native boolean nativeReportsPresentTimes(long handle);
    private static native int nativeTakePresentReports(long handle, long[] out);
    private static native String nativeGetRendererText(long handle);
    private static native String nativeGetPacingText(long handle);
    private static native void nativeStop(long handle);
    private static native void nativeDestroy(long handle);
}

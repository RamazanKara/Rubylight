package com.limelight.binding.video;

import android.annotation.SuppressLint;
import android.content.Context;
import android.content.SharedPreferences;
import android.os.Build;
import android.os.Debug;
import android.os.Process;
import android.preference.PreferenceManager;
import android.view.Surface;

import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.nvstream.jni.MoonBridge;

import java.io.File;

// Adapted from joemossjr16/artemis-android-pyrowave 387d3a5c (GPL-3.0).
public final class PyroWaveDecoderRenderer {
    private static final boolean LIBRARY_LOADED = loadLibrary();
    private long handle;
    private int format;
    private DecoderPerformanceHints performanceHints;
    private Context maxClocksContext;
    // Survives a crash, so the next stream can hand GPU power management back to the kernel.
    private static final String MAX_CLOCKS_ACTIVE = "pyrowave_max_clocks_active";

    private static boolean loadLibrary() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return false;
        if (!Process.is64Bit()) return false;
        try {
            System.loadLibrary("pyrowave-renderer");
            return true;
        } catch (UnsatisfiedLinkError e) {
            LimeLog.info("PyroWave unavailable: " + e.getMessage());
            return false;
        }
    }

    static boolean isAvailable() {
        return getReadiness(false) == 0;
    }

    private static int getReadiness(boolean hdr) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return -1;
        if (!Process.is64Bit()) return -2;
        return LIBRARY_LOADED ? nativeGetReadiness(hdr) : -3;
    }

    public static int getReadinessSummary(boolean hdr) {
        // Native reason codes are shared with the renderer's device checks.
        switch (getReadiness(hdr)) {
            case 0: return R.string.pyrowave_ready;
            case -1: return R.string.pyrowave_needs_android;
            case -2: return R.string.pyrowave_needs_64_bit;
            case -3: return R.string.pyrowave_decoder_unavailable;
            case 1: return R.string.pyrowave_needs_vulkan;
            case 2: return R.string.pyrowave_needs_subgroups;
            case 3: return R.string.pyrowave_missing_features;
            case 4: return R.string.pyrowave_missing_formats;
            case 5: return R.string.pyrowave_device_limits;
            case 6: return R.string.pyrowave_missing_queue;
            case 7: return R.string.pyrowave_missing_swapchain;
            case 8: return R.string.pyrowave_missing_hdr;
            default: return R.string.pyrowave_check_failed;
        }
    }

    /** Whether this device has an Adreno GPU, whose clocks the app can pin while streaming. */
    public static boolean hasAdrenoGpu() {
        return new File("/dev/kgsl-3d0").exists();
    }

    // SGSR serves both GPU upscaling choices: it is a single pass, cheaper than FSR's two.
    static int upscaleMode(UpscalingPolicy.Mode mode) {
        return mode == UpscalingPolicy.Mode.FSR1 || mode == UpscalingPolicy.Mode.SGSR1 ? 1 :
                mode == UpscalingPolicy.Mode.BILINEAR ? 2 : 0;
    }

    synchronized boolean setup(Surface surface, int format, int width, int height, int fps, float displayRefreshRate,
                               boolean fullRange, boolean stretch, UpscalingPolicy.Mode upscaling, int sharpness,
                               Context context, boolean maxClocks) {
        cleanup();
        if (!LIBRARY_LOADED || surface == null || !surface.isValid()) return false;
        try {
            handle = nativeCreate(surface, width, height, fps, displayRefreshRate,
                    (format & MoonBridge.VIDEO_FORMAT_MASK_YUV444) != 0,
                    (format & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0, fullRange, stretch,
                    upscaleMode(upscaling), SgsrConstants.edgeSharpness(sharpness));
        } catch (IllegalArgumentException e) {
            // The surface may be released between isValid() and the native window lookup.
            LimeLog.warning("PyroWave surface unavailable: " + e);
            return false;
        }
        this.format = handle != 0 ? format : 0;
        setMaxClocks(context, handle != 0 && maxClocks && hasAdrenoGpu());
        return handle != 0;
    }

    @SuppressLint("ApplySharedPref") // Must reach disk before a driver crash could kill the process.
    private void setMaxClocks(Context context, boolean enabled) {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
        boolean stale = prefs.getBoolean(MAX_CLOCKS_ACTIVE, false) && maxClocksContext == null;
        if (!enabled && !stale) return;
        if (!LIBRARY_LOADED) return;
        boolean applied = nativeSetGpuMaxClocks(enabled);
        maxClocksContext = enabled && applied ? context.getApplicationContext() : null;
        prefs.edit().putBoolean(MAX_CLOCKS_ACTIVE, maxClocksContext != null).commit();
    }

    synchronized int getFormat() {
        return format;
    }

    // CLOCK_MONOTONIC at the decode fence. The renderer's own thread presents the frame afterwards.
    synchronized long submitFrame(byte[] data, int length, long ptsUs, FrameLatencyStats stats,
                                  Context context, boolean enablePerformanceHints, int frameRate) {
        if (handle == 0) return -1;
        // setup() runs on the connection thread; submission runs on the actual decode thread.
        if (performanceHints == null) {
            performanceHints = new DecoderPerformanceHints(context, enablePerformanceHints, frameRate);
        }
        long workStartNs = Debug.threadCpuTimeNanos();
        long outputNs = 0;
        try {
            outputNs = nativeSubmitFrame(handle, data, length, ptsUs);
        } finally {
            // CPU hints must exclude the native waits for free planes and GPU completion.
            performanceHints.reportWorkDuration(Debug.threadCpuTimeNanos() - workStartNs, frameRate);
        }
        if (outputNs > 0) {
            stats.onDecoderOutput(outputIndex(ptsUs), ptsUs, outputNs);
        }
        // Keep output registration and polling under the same lock: the present thread may
        // already have shown (or replaced) this frame.
        nativePollRenderedFrames(handle, stats);
        return outputNs;
    }

    /** Frames can wait for the display while newer ones decode, so each needs its own index. */
    static int outputIndex(long ptsUs) {
        return (int) ptsUs;  // Matches the native side's release events.
    }

    synchronized void pollRenderedFrames(FrameLatencyStats stats) {
        if (handle != 0) nativePollRenderedFrames(handle, stats);
    }

    /**
     * Average ms per frame the decoder waited for free planes and the present thread waited for a
     * swapchain image, and the percentage of decoded frames a newer one replaced before display.
     */
    synchronized float[] getWaits() {
        float[] waits = handle != 0 ? nativeGetWaits(handle) : null;
        return waits != null && waits.length >= 3 ? waits : new float[] {-1, -1, -1};
    }

    synchronized int getLastGpuDecodeUs() {
        return handle != 0 ? nativeGetLastGpuDecodeUs(handle) : 0;
    }

    synchronized String getDriver() {
        return handle != 0 ? nativeGetDriver(handle) : "";
    }

    synchronized UpscalingPolicy.Reason getUpscalingReason() {
        if (handle == 0) return UpscalingPolicy.Reason.DISABLED;
        switch (nativeGetUpscalingReason(handle)) {
            case 0: return UpscalingPolicy.Reason.NONE;
            case 1: return UpscalingPolicy.Reason.DISABLED;
            case 2: return UpscalingPolicy.Reason.HDR;
            case 3: return UpscalingPolicy.Reason.NO_UPSCALE;
            default: return UpscalingPolicy.Reason.GPU_ERROR;
        }
    }

    synchronized boolean hasMaxClocks() {
        return maxClocksContext != null;
    }

    synchronized String getPresentMode() {
        return handle != 0 ? nativeGetPresentMode(handle) : "unavailable";
    }

    synchronized float getLastRecordLossPercent() {
        return handle != 0 ? nativeGetLastRecordLossPercent(handle) : 0;
    }

    synchronized void setHdrMode(boolean enabled, byte[] metadata) {
        if (handle != 0) nativeSetHdrMode(handle, enabled, metadata);
    }

    synchronized void cleanup() {
        if (performanceHints != null) {
            performanceHints.close();
            performanceHints = null;
        }
        if (handle != 0) nativeDestroy(handle);
        handle = 0;
        format = 0;
        restoreClocks();
    }

    /** Hands GPU clocks back to the driver if this stream forced them to maximum. */
    synchronized void restoreClocks() {
        if (maxClocksContext == null) return;
        Context context = maxClocksContext;
        nativeSetGpuMaxClocks(false);
        maxClocksContext = null;
        PreferenceManager.getDefaultSharedPreferences(context).edit().putBoolean(MAX_CLOCKS_ACTIVE, false).apply();
    }

    private static native int nativeGetReadiness(boolean hdr);
    private static native long nativeCreate(Surface surface, int width, int height, int fps, float displayRefreshRate,
                                           boolean chroma444, boolean tenBit, boolean fullRange, boolean stretch,
                                           int upscale, float edgeSharpness);
    private static native String nativeGetDriver(long handle);
    private static native int nativeGetUpscalingReason(long handle);
    private static native boolean nativeSetGpuMaxClocks(boolean enabled);
    private static native long nativeSubmitFrame(long handle, byte[] data, int length, long ptsUs);
    private static native boolean nativePollRenderedFrames(long handle, FrameLatencyStats stats);
    private static native int nativeGetLastGpuDecodeUs(long handle);
    private static native float[] nativeGetWaits(long handle);
    private static native String nativeGetPresentMode(long handle);
    private static native float nativeGetLastRecordLossPercent(long handle);
    private static native void nativeSetHdrMode(long handle, boolean enabled, byte[] metadata);
    private static native void nativeDestroy(long handle);
}

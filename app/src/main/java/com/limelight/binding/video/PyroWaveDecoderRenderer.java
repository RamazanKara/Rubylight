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
import java.nio.ByteBuffer;
import java.util.Locale;

// Adapted from joemossjr16/artemis-android-pyrowave 387d3a5c (GPL-3.0).
public final class PyroWaveDecoderRenderer {
    private static final boolean LIBRARY_LOADED = loadLibrary();
    private long handle;
    // Frame Warp for this stream (setFrameWarp), until cleanup
    private boolean frameWarp;
    private int format;
    private DecoderPerformanceHints performanceHints;
    private Thread submissionThread;
    private Context maxClocksContext;
    private Context driverContext;
    private GpuDriverStore.Driver customDriver;
    private String driverStatus = "";
    private long lastSubmissionDurationNs;
    private final EarlyWork earlyWork = new EarlyWork();
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

    /** 0 when PyroWave can run on this device, else the string that says why it cannot. */
    static int unavailableReason(Context context) {
        int readiness = getReadiness(context, false);
        return readiness == 0 ? 0 : failureSummary(readiness);
    }

    /**
     * The string that says why this device can never run PyroWave, whatever its GPU driver, or 0
     * when the GPU decides (Android 10, a 64-bit app and the renderer library are there).
     */
    public static int neverAvailableReason() {
        int readiness = appReadiness();
        return readiness == 0 ? 0 : failureSummary(readiness);
    }

    private static int appReadiness() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return -1;
        if (!Process.is64Bit()) return -2;
        if (!LIBRARY_LOADED) return -3;
        return 0;
    }

    private static int getReadiness(Context context, boolean hdr) {
        int app = appReadiness();
        if (app != 0) return app;
        GpuDriverStore.Driver driver = GpuDriverStore.isSupported() ? GpuDriverStore.selected(context) : null;
        if (driver != null && GpuDriverStore.beginUse(context, driver)) {
            int result = nativeGetReadiness(hdr, context.getApplicationInfo().nativeLibraryDir,
                    driver.directory.getAbsolutePath() + File.separator, driver.libraryName);
            GpuDriverStore.endUse(context, driver, result != 0);
            if (result == 0) return 0;
            LimeLog.warning("Custom GPU driver cannot decode PyroWave; checking the system driver");
        }
        return nativeGetReadiness(hdr, null, null, null);
    }

    public static int getReadinessSummary(Context context, boolean hdr) {
        int readiness = getReadiness(context, hdr);
        if (readiness == 0) {
            return GpuDriverStore.usedFallback(context) ? R.string.gpu_driver_system_fallback : R.string.pyrowave_ready;
        }
        return failureSummary(readiness);
    }

    /**
     * Why PyroWave HDR cannot run here, after its output surface failed. A device whose HDR
     * readiness check passes still failed to create the 10-bit HDR10 swapchain.
     */
    public static int hdrFailureSummary(Context context) {
        int readiness = getReadiness(context, true);
        return readiness == 0 ? surfaceFailureSummary(nativeGetHdrSurfaceFailure(), Build.VERSION.SDK_INT)
                : failureSummary(readiness);
    }

    /**
     * Android 13 and 14 list no HDR10 surface format; HDR is then sent through the window dataspace.
     * Code 1 is no usable 10-bit format at all, 2 is the display or driver refusing that dataspace,
     * 3 the same for an HLG stream, 4 an HLG window that no longer holds it after the first frame.
     */
    static int surfaceFailureSummary(int nativeFailure, int sdkInt) {
        if (nativeFailure == 1 && sdkInt < 35) return R.string.pyrowave_hdr_needs_android_15;
        if (nativeFailure == 2) return R.string.pyrowave_hdr_display_refused;
        if (nativeFailure == 3 || nativeFailure == 4) return R.string.pyrowave_hlg_display_refused;
        return R.string.pyrowave_missing_swapchain;
    }

    /** Why the running stream's HDR output failed, 0 when it did not (a refused HDR10 or HLG dataspace). */
    static int streamHdrFailure() {
        if (!LIBRARY_LOADED) return 0;
        int failure = nativeGetHdrSurfaceFailure();
        return failure >= 2 && failure <= 4 ? surfaceFailureSummary(failure, Build.VERSION.SDK_INT) : 0;
    }

    /** The running HLG stream's window refused BT2020_HLG or no longer holds it (codes 3 and 4). */
    static boolean hlgWindowLost() {
        if (!LIBRARY_LOADED) return false;
        int failure = nativeGetHdrSurfaceFailure();
        return failure == 3 || failure == 4;
    }

    private static int failureSummary(int readiness) {
        // Native reason codes are shared with the renderer's device checks.
        switch (readiness) {
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

    /**
     * pacingMode is FramePacingPolicy.pyroWaveMode's. topRefreshRate is the display's highest refresh
     * rate, read only with the PC's frame times (Real VRR): frames are then shown no closer than one
     * refresh at it. 0 leaves that open.
     */
    synchronized boolean setup(Surface surface, int format, int width, int height, int fps, float displayRefreshRate,
                               float topRefreshRate, int pacingMode, boolean fullRange, boolean stretch,
                               UpscalingPolicy.Mode upscaling, int sharpness,
                               Context context, boolean maxClocks, boolean preacquire) {
        cleanup();
        if (!LIBRARY_LOADED || surface == null || !surface.isValid()) return false;
        driverContext = context.getApplicationContext();
        customDriver = GpuDriverStore.isSupported() ? GpuDriverStore.selected(context) : null;
        boolean requestedCustom = customDriver != null;
        for (int attempt = 0; attempt < (requestedCustom ? 2 : 1); attempt++) {
            if (customDriver != null && !GpuDriverStore.beginUse(context, customDriver)) customDriver = null;
            try {
                handle = nativeCreate(surface, width, height, fps, displayRefreshRate, topRefreshRate, pacingMode,
                        (format & MoonBridge.VIDEO_FORMAT_MASK_YUV444) != 0,
                        (format & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0, fullRange, stretch,
                        upscaleMode(upscaling), SgsrConstants.edgeSharpness(sharpness), preacquire,
                        customDriver != null ? context.getApplicationInfo().nativeLibraryDir : null,
                        customDriver != null ? customDriver.directory.getAbsolutePath() + File.separator : null,
                        customDriver != null ? customDriver.libraryName : null);
            } catch (IllegalArgumentException e) {
                LimeLog.warning("PyroWave surface unavailable: " + e);
            }
            if (handle != 0) break;
            if (customDriver != null) {
                GpuDriverStore.endUse(context, customDriver, true);
                customDriver = null;
                LimeLog.warning("Custom GPU driver initialization failed; retrying the system driver");
            }
        }
        driverStatus = customDriver != null ? customDriver.name : context.getString(
                requestedCustom || GpuDriverStore.usedFallback(context) ? R.string.gpu_driver_system_fallback : R.string.gpu_driver_system);
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

    /**
     * Draws Frame Warp (docs/frame-warp.md) with the table at {@code api} (MoonBridge.frameWarpApi()) from the
     * next picture on. The renderer is set up before the stream says whether it offers Frame Warp, so this comes
     * after setup(). False when it cannot; the stream then goes without.
     */
    synchronized boolean setFrameWarp(long api) {
        frameWarp = handle != 0 && api != 0 && nativeSetFrameWarp(handle, api);
        return frameWarp;
    }

    static final int SUBMIT_ACCEPTED = 1;
    static final int SUBMIT_BUSY = 2;

    // Accepted and busy are distinct from invalid input (zero) and a fatal error (negative). Completion
    // timestamps arrive separately through nativePollRenderedFrames, before release/present events.
    // Exactly one of data and directData is set: directData is the Rust engine's frame in place,
    // read by native code during the call without a copy.
    synchronized long submitFrame(byte[] data, ByteBuffer directData, int length, long ptsUs, int frameNumber,
                                  long hostPresentationTimeUs, PyroWaveDisplayClock.Snapshot displayClock,
                                  FrameLatencyStats stats, Context context, boolean enablePerformanceHints, int frameRate) {
        lastSubmissionDurationNs = 0;
        if (handle == 0) return -1;
        // setup() runs on the connection thread; submission runs on the actual decode thread.
        // Track the Java thread identity because native thread IDs can be reused after a restart.
        Thread currentSubmissionThread = Thread.currentThread();
        if (submissionThread != currentSubmissionThread) {
            if (!setSubmissionThreadPriority(Process.THREAD_PRIORITY_URGENT_DISPLAY) &&
                    !setSubmissionThreadPriority(Process.THREAD_PRIORITY_DISPLAY)) {
                LimeLog.warning("PyroWave submission thread priority could not be raised");
            }
            if (performanceHints != null) {
                performanceHints.close();
                performanceHints = null;
            }
            submissionThread = currentSubmissionThread;
        }
        if (performanceHints == null) {
            performanceHints = new DecoderPerformanceHints(context, enablePerformanceHints, frameRate);
        }
        // The host clock drives cadence; local ptsUs remains the unique statistics key.
        // Copy the latest measured display phase before starting the native submission timer.
        long vsyncNs = displayClock.vsyncNs;
        long intervalNs = displayClock.intervalNs;
        // Frame Warp shifts each picture for the frame the host drew it as
        if (frameWarp) nativeNoteFrameNumber(handle, frameNumber);
        long workStartNs = Debug.threadCpuTimeNanos();
        long result;
        long submissionStartNs = System.nanoTime();
        try {
            result = directData != null ?
                    nativeSubmitDirectFrame(handle, directData, length, ptsUs, hostPresentationTimeUs, vsyncNs, intervalNs) :
                    nativeSubmitFrame(handle, data, length, ptsUs, hostPresentationTimeUs, vsyncNs, intervalNs);
        } finally {
            // Match Moonlight X's wall-clock interval around native parsing, command recording
            // and submission. Polling and Android performance-hint work are not submission work.
            lastSubmissionDurationNs = System.nanoTime() - submissionStartNs;
            performanceHints.reportWorkDuration(earlyWork.take(Debug.threadCpuTimeNanos() - workStartNs), frameRate);
        }
        nativePollRenderedFrames(handle, stats);
        return result;
    }

    private static boolean setSubmissionThreadPriority(int priority) {
        try {
            // Keep a stronger priority if the caller already assigned one. These are ordinary
            // scheduler nice values, matching the native presentation and completion workers.
            if (Process.getThreadPriority(Process.myTid()) > priority) {
                Process.setThreadPriority(priority);
            }
            return true;
        } catch (IllegalArgumentException | SecurityException e) {
            // Some devices deny the stronger priority. Try the fallback once, then keep decoding.
            return false;
        }
    }

    synchronized long getLastSubmissionDurationNs() {
        return lastSubmissionDurationNs;
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
     * swapchain image, the percentage of decoded frames a newer one replaced before display, and,
     * with the PC's frame times (Real VRR), the average ms a ready frame was held for its time
     * (-1 in every other mode).
     */
    synchronized float[] getWaits() {
        float[] waits = handle != 0 ? nativeGetWaits(handle) : null;
        return waits != null && waits.length >= 4 ? waits : new float[] {-1, -1, -1, -1};
    }

    /** The overlay line for {@link #getWaits()}'s hold with the PC's frame times, or null without one. */
    static String hostTimesLine(float heldMs) {
        return heldMs >= 0 && Float.isFinite(heldMs) ?
                String.format(Locale.US, "PC frame times: held %.1f ms", heldMs) : null;
    }

    synchronized int getLastGpuDecodeUs() {
        return handle != 0 ? nativeGetLastGpuDecodeUs(handle) : 0;
    }

    synchronized String getDriver() {
        return handle != 0 ? nativeGetDriver(handle) + " · " + driverStatus : "";
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

    synchronized void submitChunks(byte[] data, int length, boolean first) {
        if (handle == 0) return;
        long workStartNs = Debug.threadCpuTimeNanos();
        try {
            nativeSubmitChunks(handle, data, length, first);
        } finally {
            earlyWork.add(Debug.threadCpuTimeNanos() - workStartNs);
        }
    }

    // The Rust engine's chunk batch in place: data is a direct buffer, read only during the call.
    synchronized void submitChunksDirect(ByteBuffer data, int length, boolean first) {
        if (handle == 0) return;
        long workStartNs = Debug.threadCpuTimeNanos();
        try {
            nativeSubmitChunksDirect(handle, data, length, first);
        } finally {
            earlyWork.add(Debug.threadCpuTimeNanos() - workStartNs);
        }
    }

    /**
     * Thread CPU time of the early record pushes since the last frame's submit. They are that
     * frame's parse work, done while it arrived, so the next performance-hint report includes
     * them; otherwise the hint session would see only the shorter submit and lower the clocks.
     */
    static final class EarlyWork {
        private long pendingNs;

        void add(long ns) {
            pendingNs += ns;
        }

        /** A frame's submit time plus the pushes before it; the pushes start over. */
        long take(long submitNs) {
            long total = submitNs + pendingNs;
            pendingNs = 0;
            return total;
        }
    }

    /**
     * Whether a stream asks for early record push: a PyroWave stream from a host with record
     * framing hands the renderer each frame's packets while they arrive, so its records are in
     * the decoder by the frame's last packet. allowed is {@link #earlyRecordsAllowed()}.
     */
    public static boolean wantsEarlyRecords(boolean pyroWave, boolean hostRecords, boolean allowed) {
        return pyroWave && hostRecords && allowed;
    }

    /** On unless debug.nightjar.pyro_early is 0 (A/B runs against the whole-frame push). */
    public static boolean earlyRecordsAllowed() {
        return LIBRARY_LOADED && nativeEarlyRecordsAllowed();
    }

    /**
     * Early record push over the last stats interval: the percentage of frames whose records were
     * in the decoder by their last packet (-1 when none came early), and the frames whose early
     * packets fell short and were pushed whole.
     */
    synchronized float[] getEarlyRecords() {
        float[] early = handle != 0 ? nativeGetEarlyRecords(handle) : null;
        return early != null && early.length >= 2 ? early : new float[] {-1, 0};
    }

    /** The overlay line for {@link #getEarlyRecords()}; a frame that fell back is counted. */
    static String earlyRecordsLine(float[] early) {
        if (early[0] < 0) return "Early record push: no frame came early";
        String line = String.format(Locale.US, "Early record push: %.0f%% of frames", early[0]);
        return early[1] > 0 ? line + String.format(Locale.US, " | %.0f pushed whole", early[1]) : line;
    }

    synchronized float getLastRecordLossPercent() {
        return handle != 0 ? nativeGetLastRecordLossPercent(handle) : 0;
    }

    synchronized void setHdrMode(boolean enabled, byte[] metadata) {
        if (handle != 0) nativeSetHdrMode(handle, enabled, metadata);
    }

    /** The stream's HDR transfer from its ANNOUNCE: HLG, else PQ. Before the first frame. */
    synchronized void setTransfer(boolean hlg) {
        if (handle != 0) nativeSetTransfer(handle, hlg);
    }

    synchronized void cleanup() {
        cleanup(null);
    }

    synchronized void cleanup(FrameLatencyStats stats) {
        submissionThread = null;
        frameWarp = false;
        earlyWork.take(0);
        if (performanceHints != null) {
            performanceHints.close();
            performanceHints = null;
        }
        // Join the completion/presentation workers and deliver their final events before the
        // native renderer is deleted. The caller then finalizes any genuinely unfinished frames.
        // Clear ownership first: a statistics callback can throw after native destruction.
        long renderer = handle;
        handle = 0;
        format = 0;
        try {
            if (renderer != 0) nativeDestroy(renderer, stats);
        } finally {
            if (customDriver != null) {
                GpuDriverStore.endUse(driverContext, customDriver, false);
                customDriver = null;
            }
            driverContext = null;
            restoreClocks();
        }
    }

    /** Hands GPU clocks back to the driver if this stream forced them to maximum. */
    synchronized void restoreClocks() {
        if (maxClocksContext == null) return;
        Context context = maxClocksContext;
        nativeSetGpuMaxClocks(false);
        maxClocksContext = null;
        PreferenceManager.getDefaultSharedPreferences(context).edit().putBoolean(MAX_CLOCKS_ACTIVE, false).apply();
    }

    private static native int nativeGetHdrSurfaceFailure();
    private static native int nativeGetReadiness(boolean hdr, String hooks, String directory, String name);
    private static native long nativeCreate(Surface surface, int width, int height, int fps, float displayRefreshRate,
                                           float topRefreshRate, int pacingMode,
                                           boolean chroma444, boolean tenBit, boolean fullRange, boolean stretch,
                                           int upscale, float edgeSharpness, boolean preacquire,
                                           String hooks, String directory, String name);
    private static native String nativeGetDriver(long handle);
    private static native int nativeGetUpscalingReason(long handle);
    private static native boolean nativeSetGpuMaxClocks(boolean enabled);
    private static native long nativeSubmitFrame(long handle, byte[] data, int length, long ptsUs,
                                                long hostPresentationTimeUs, long vsyncNs, long intervalNs);
    private static native long nativeSubmitDirectFrame(long handle, ByteBuffer data, int length, long ptsUs,
                                                      long hostPresentationTimeUs, long vsyncNs, long intervalNs);
    private static native boolean nativePollRenderedFrames(long handle, FrameLatencyStats stats);
    private static native int nativeGetLastGpuDecodeUs(long handle);
    private static native float[] nativeGetWaits(long handle);
    private static native String nativeGetPresentMode(long handle);
    private static native float nativeGetLastRecordLossPercent(long handle);
    private static native void nativeSubmitChunks(long handle, byte[] data, int length, boolean first);
    private static native void nativeSubmitChunksDirect(long handle, ByteBuffer data, int length, boolean first);
    private static native boolean nativeEarlyRecordsAllowed();
    private static native float[] nativeGetEarlyRecords(long handle);
    private static native void nativeSetHdrMode(long handle, boolean enabled, byte[] metadata);
    private static native void nativeSetTransfer(long handle, boolean hlg);
    private static native boolean nativeSetFrameWarp(long handle, long api);
    private static native void nativeNoteFrameNumber(long handle, int frame);
    private static native void nativeDestroy(long handle, FrameLatencyStats stats);
}

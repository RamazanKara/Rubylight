package com.limelight.binding.video;

import com.limelight.preferences.PreferenceConfiguration;

final class FramePacingPolicy {
    // Keep a renderer turn finite even if decoder output keeps arriving while we catch up.
    static final int MAX_OUTPUT_DRAIN = 64;

    /**
     * The PyroWave presenter's mode for Real VRR, beside the frame pacing values it takes as they
     * are: each frame at the time the shared present clock gives it (present_timing.h, HostTimes).
     */
    static final int PYROWAVE_HOST_TIMES = 4;

    /**
     * The mode the PyroWave presenter runs in. Variable refresh rate replaces frame pacing. With
     * Real VRR on a screen that refreshes when a frame comes (the conditions under which
     * HostFrameClock schedules MediaCodec frames for an adaptive refresh rate), each frame is
     * shown at its time on the PC plus a steady delay. Otherwise VRR shows frames as they arrive:
     * with Real VRR off, and on a screen with a fixed refresh, whose refresh times a frame's time
     * cannot move.
     *
     * @param hostFrameTimes the stream's host frame times switch (PreferenceConfiguration.hostFrameTimes),
     *                       which under VRR is Real VRR
     * @param adaptive       the stream's display refreshes when a frame comes (Android 16 ARR)
     */
    static int pyroWaveMode(int requestedMode, boolean vrr, boolean hostFrameTimes, boolean adaptive) {
        if (!vrr) return requestedMode;
        return hostFrameTimes && adaptive ? PYROWAVE_HOST_TIMES : PreferenceConfiguration.FRAME_PACING_MIN_LATENCY;
    }

    /**
     * Whether the PyroWave presenter needs the display's vsync callbacks: only for the modes that
     * put frames onto the vsync grid. Under VRR it shows frames as they arrive or at the PC's
     * frame times, on no grid.
     */
    static boolean usesPyroWaveDisplayClock(int requestedMode, boolean vrr, boolean localDisplay) {
        return !vrr && localDisplay && (requestedMode == PreferenceConfiguration.FRAME_PACING_BALANCED ||
                requestedMode == PreferenceConfiguration.FRAME_PACING_CAP_FPS ||
                requestedMode == PreferenceConfiguration.FRAME_PACING_MAX_SMOOTHNESS);
    }

    static boolean dropQueuedFrames(int mode) {
        return mode != PreferenceConfiguration.FRAME_PACING_MAX_SMOOTHNESS &&
                mode != PreferenceConfiguration.FRAME_PACING_CAP_FPS;
    }

    static boolean isFrameDue(long dueNs, long vsyncNs, long vsyncIntervalNs) {
        return vsyncNs + vsyncIntervalNs / 2 >= dueNs;
    }

    /**
     * Whether Balanced holds decoded frames for the display's vsync callback (issue 7). That aligns
     * a stream slower than the panel to its cadence (60 fps on 120 Hz). When the stream is about as
     * fast as the panel, or faster, every refresh is due anyway: outputs then land next to the
     * callback, jitter puts two between one pair of callbacks and none between the next, and one
     * of the two was dropped (6% at 120 fps on a 120 Hz Retroid, 10% at 60 on a 60 Hz Shield).
     * Those streams are released as they come out, as in Lowest latency, but only to a host that
     * takes phase lock reports: their slack moves the host's capture so outputs land ahead of the
     * latch. Any other host keeps the vsync callback and its queue, and so does Drop late frames
     * off, which asks for that queue. Without a known refresh interval the callback keeps pacing.
     */
    static boolean balancedPacesOnVsync(boolean hostPhaseLock, boolean dropLateFrames, int fpsMilliHz,
                                        long vsyncIntervalNs) {
        if (!hostPhaseLock || !dropLateFrames) return true;
        // A refresh rate of 0 gives no interval or an unbounded one; nothing refreshes slower than 1 Hz.
        if (vsyncIntervalNs <= 0 || vsyncIntervalNs > 1_000_000_000L) return true;
        return frameIntervalNs(fpsMilliHz) * 20 > vsyncIntervalNs * 21;
    }

    static long nextFrameTimeNs(long dueNs, long vsyncNs, int fps) {
        return nextFrameTimeNsMilliHz(dueNs, vsyncNs, fps * 1000);
    }

    static long nextFrameTimeNsMilliHz(long dueNs, long vsyncNs, int fpsMilliHz) {
        long intervalNs = frameIntervalNs(fpsMilliHz);
        if (dueNs == 0) return vsyncNs + intervalNs;
        return dueNs + ((vsyncNs - dueNs) / intervalNs + 1) * intervalNs;
    }

    static long frameIntervalNs(int fpsMilliHz) {
        if (fpsMilliHz < 1000 || fpsMilliHz > 1_000_000) {
            throw new IllegalArgumentException("Invalid stream frame rate");
        }
        return 1_000_000_000_000L / fpsMilliHz;
    }

    static int negotiatedFrameRateMilliHz(int requestedMilliHz, int negotiatedFps) {
        return requestedMilliHz >= 1000 && requestedMilliHz <= 1_000_000 &&
                (requestedMilliHz + 500) / 1000 == negotiatedFps ? requestedMilliHz : negotiatedFps * 1000;
    }

    /** Whether the present scheduler places frames: host frame times in latency mode only. */
    static boolean schedulesFrames(boolean hostFrameTimes, int framePacing) {
        return hostFrameTimes && framePacing != PreferenceConfiguration.FRAME_PACING_BALANCED &&
                framePacing != PreferenceConfiguration.FRAME_PACING_MAX_SMOOTHNESS &&
                framePacing != PreferenceConfiguration.FRAME_PACING_CAP_FPS;
    }

    /** Whether decoder debt is recovered (DecoderQueuePolicy.Debt): Smoothest and the frame rate limit. */
    static boolean recoversDecoderDebt(boolean hostFrameTimes, int framePacing) {
        return !schedulesFrames(hostFrameTimes, framePacing) && !dropQueuedFrames(framePacing);
    }

    static boolean shouldDrainOutput(int mode, long enqueueNs, long nowNs, int fpsMilliHz, int drained) {
        if (drained >= MAX_OUTPUT_DRAIN) return false;
        if (dropQueuedFrames(mode)) return true;
        // Keep one frame of jitter buffering, then catch up only if a newer decoded output is
        // available. A 50 ms floor left 30–40 ms of persistent decoder debt untouched at 60 FPS.
        // The low-rate ceiling stays finite. Never use unknown/future timestamps or discard
        // compressed reference frames; a stale last output still gets displayed.
        long maximumAgeNs = Math.min(250_000_000L, frameIntervalNs(fpsMilliHz));
        return enqueueNs > 0 && nowNs > enqueueNs && nowNs - enqueueNs > maximumAgeNs;
    }

    static boolean hasOutputDrainTimeRemaining(long elapsedNs, int fpsMilliHz) {
        // A slow discard can otherwise keep chasing newly decoded outputs without ever
        // presenting. One codec call can exceed this budget; after it returns, hand off the
        // output we already own instead of beginning another discard. Retain the count cap too.
        return elapsedNs >= 0 && elapsedNs < frameIntervalNs(fpsMilliHz);
    }
}

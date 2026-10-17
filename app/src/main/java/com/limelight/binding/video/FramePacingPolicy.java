package com.limelight.binding.video;

import com.limelight.preferences.PreferenceConfiguration;

final class FramePacingPolicy {
    // Keep a renderer turn finite even if decoder output keeps arriving while we catch up.
    static final int MAX_OUTPUT_DRAIN = 64;

    static int pyroWaveMode(int requestedMode, boolean vrr) {
        return vrr ? PreferenceConfiguration.FRAME_PACING_MIN_LATENCY : requestedMode;
    }

    static boolean usesPyroWaveDisplayClock(int requestedMode, boolean vrr, boolean localDisplay) {
        int mode = pyroWaveMode(requestedMode, vrr);
        return localDisplay && (mode == PreferenceConfiguration.FRAME_PACING_BALANCED ||
                mode == PreferenceConfiguration.FRAME_PACING_CAP_FPS ||
                mode == PreferenceConfiguration.FRAME_PACING_MAX_SMOOTHNESS);
    }

    static boolean dropQueuedFrames(int mode) {
        return mode != PreferenceConfiguration.FRAME_PACING_MAX_SMOOTHNESS &&
                mode != PreferenceConfiguration.FRAME_PACING_CAP_FPS;
    }

    static boolean isFrameDue(long dueNs, long vsyncNs, long vsyncIntervalNs) {
        return vsyncNs + vsyncIntervalNs / 2 >= dueNs;
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

    static boolean shouldDrainOutput(int mode, long enqueueNs, long nowNs, int fpsMilliHz, int drained) {
        if (drained >= MAX_OUTPUT_DRAIN) return false;
        if (dropQueuedFrames(mode)) return true;
        // Preserve ordinary smooth playback. Only an output already older than three frames
        // (at least 50 ms, at most 250 ms) may be replaced, and only if another decoded output
        // is available. Never use unknown/future timestamps or discard compressed references.
        long maximumAgeNs = Math.max(50_000_000L, Math.min(250_000_000L, frameIntervalNs(fpsMilliHz) * 3));
        return enqueueNs > 0 && nowNs > enqueueNs && nowNs - enqueueNs > maximumAgeNs;
    }
}

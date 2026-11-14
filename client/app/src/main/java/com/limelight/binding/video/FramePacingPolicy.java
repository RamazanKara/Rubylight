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

    /** How far a refresh may be from a whole multiple of the stream's rate, per multiple. */
    static final double RATE_TOLERANCE_HZ = 0.5;
    /** The most refreshes fractional V-Sync holds a frame for. */
    static final int MAX_HELD_REFRESHES = 4;

    /**
     * Fractional V-Sync: the refreshes each frame stays on a display that refreshes at a whole
     * multiple of the stream's rate (2 for 60 fps on 120 Hz), else 1. VRR and the setting off take
     * 1 too. Java port of nightjar_policy::vsync::refreshes_per_frame (client/protocol/policy).
     */
    static int refreshesPerFrame(double refreshHz, double fps, boolean vrr, boolean fractional) {
        if (!fractional || vrr || !(refreshHz > 0) || !(fps > 0)) return 1;
        for (int k = 1; k <= MAX_HELD_REFRESHES; k++) {
            if (Math.abs(refreshHz - fps * k) <= RATE_TOLERANCE_HZ * k) return k;
        }
        return 1;
    }

    /**
     * The time to stamp a frame held for {@code refreshes} refreshes with (fractional V-Sync): the
     * refresh {@code refreshes} after the last frame's ({@code lastTargetNs}), or the first one it
     * can still make ({@code earliestNs}, the next refresh past the compositor's deadline) when it
     * comes later than that, which the next frames then count from. The compositor shows a frame
     * at the refresh its time asks for, so a frame that comes early waits for its turn instead of
     * cutting the last one's short. The turn is put on the refresh grid of {@code vsyncNs}, so a
     * rounded interval never drifts off it.
     */
    static long heldTargetNs(long earliestNs, long lastTargetNs, long vsyncNs, long intervalNs, int refreshes) {
        if (refreshes <= 1 || intervalNs <= 0 || lastTargetNs == 0) return earliestNs;
        long turnNs = lastTargetNs + refreshes * intervalNs;
        long offsetNs = turnNs - vsyncNs;
        turnNs = vsyncNs + Math.floorDiv(offsetNs + intervalNs / 2, intervalNs) * intervalNs;
        return Math.max(earliestNs, turnNs);
    }

    /**
     * Fractional V-Sync: frames released {@link #EARLY_SPARE_NS} ahead of the deadline of the
     * refresh before their turn for {@link #EARLY_FOR_NS} in a row move the turns a refresh earlier,
     * once. A source whose clock runs ahead of the display brings them earlier and earlier, each
     * waiting longer, up to the whole hold, until a host's phase lock moves them later, and a host
     * without one never does. Longer than the lock takes (a report or two), so the two never take
     * turns. The desktop's phase::HeldAnchor::hold does the same.
     */
    static final long EARLY_FOR_NS = 2_000_000_000L;
    static final long EARLY_SPARE_NS = 3_000_000L;

    /**
     * Whether a frame released at {@code releaseNs} makes the refresh before its turn
     * ({@code turnNs}), whose compositor deadline comes {@code deadlineNs} ahead of it, with
     * {@link #EARLY_SPARE_NS} to spare.
     */
    static boolean earlyForTurn(long releaseNs, long turnNs, long intervalNs, long deadlineNs) {
        return intervalNs > 0 && turnNs - intervalNs - deadlineNs - releaseNs >= EARLY_SPARE_NS;
    }

    /**
     * The refresh grid a phase report measures slack to with fractional V-Sync: a refresh of the
     * grid ({@code anchorNs}) put {@code periods} grid periods into the past, so a report never
     * counts from a refresh still to come, and the grid's period; the display's own refresh and
     * interval without a hold. Returns {anchor, period}.
     */
    static long[] heldReportGrid(long vsyncNs, long intervalNs, long anchorNs, int refreshes) {
        if (refreshes <= 1 || anchorNs == 0 || intervalNs <= 0) return new long[] {vsyncNs, intervalNs};
        long periodNs = intervalNs * refreshes;
        return new long[] {anchorNs - 4 * periodNs, periodNs};
    }

    static boolean hasOutputDrainTimeRemaining(long elapsedNs, int fpsMilliHz) {
        // A slow discard can otherwise keep chasing newly decoded outputs without ever
        // presenting. One codec call can exceed this budget; after it returns, hand off the
        // output we already own instead of beginning another discard. Retain the count cap too.
        return elapsedNs >= 0 && elapsedNs < frameIntervalNs(fpsMilliHz);
    }
}

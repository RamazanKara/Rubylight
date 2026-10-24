package com.limelight.binding.video;

/**
 * Experimental: the order a stream ends in on an Android TV that is showing HDR.
 *
 * <p>Some TVs (NVIDIA Shield on an HDR screen) lose the HDMI signal when the HDR to SDR switch and
 * the display mode revert overlap. So the exit is staged: stop the connection and release the
 * video output (HDR turns off), wait until the display stops reporting changes, give the display
 * mode back ourselves, wait again, and only then finish the activity. Every wait is capped.
 *
 * <p>This class only decides; the activity does the work and calls {@link #poll} on its UI
 * thread every {@link #POLL_MS}. Not thread safe. Phones, tablets and SDR streams never use it.
 */
public final class StagedStreamExit {
    public static final long POLL_MS = 50;
    /** The most the connection stop (decoder stop and release) may take before the exit goes on. */
    public static final long STOP_TIMEOUT_MS = 2000;
    /** The display must have been quiet this long after the HDR output went away... */
    public static final long DISPLAY_QUIET_MS = 750;
    /** ...but the wait never lasts longer than this. */
    public static final long DISPLAY_SETTLE_MAX_MS = 2500;
    /** The same two numbers for the wait after the display mode was given back. */
    public static final long MODE_QUIET_MS = 500;
    public static final long MODE_SETTLE_MAX_MS = 1500;

    public enum Stage { IDLE, STOPPING, DISPLAY_SETTLE, MODE_SETTLE, DONE }

    public enum Action {
        WAIT,
        /** The stop is over (or timed out): tear the video surface down so the HDR output ends. */
        RELEASE_OUTPUT,
        /** The display is quiet (or the wait ran out): clear the window's display mode request. */
        RESTORE_DISPLAY_MODE,
        /** Everything is settled (or the wait ran out): finish the activity. Returned once. */
        FINISH
    }

    private Stage stage = Stage.IDLE;
    private long stageStartMs;
    private long lastDisplayChangeMs;
    private boolean stopFinished;
    private boolean stopTimedOut;

    /** Only an Android TV that is streaming HDR takes the staged exit. */
    public static boolean applies(boolean androidTv, boolean hdr) {
        return androidTv && hdr;
    }

    /** Starts the exit; false if it already started (a second Back or quit during the exit). */
    public boolean begin(long nowMs) {
        if (stage != Stage.IDLE) return false;
        stage = Stage.STOPPING;
        stageStartMs = nowMs;
        lastDisplayChangeMs = nowMs;
        return true;
    }

    public boolean started() {
        return stage != Stage.IDLE;
    }

    public Stage stage() {
        return stage;
    }

    /** True if the connection stop was given up on after {@link #STOP_TIMEOUT_MS}. */
    public boolean stopTimedOut() {
        return stopTimedOut;
    }

    /** The connection stop thread has ended (the decoder is stopped and released). */
    public void onStopFinished() {
        stopFinished = true;
    }

    /** The video display reported a change; it is not settled until it stops doing that. */
    public void onDisplayChanged(long nowMs) {
        if (stage == Stage.DISPLAY_SETTLE || stage == Stage.MODE_SETTLE) lastDisplayChangeMs = nowMs;
    }

    public Action poll(long nowMs) {
        switch (stage) {
            case STOPPING:
                if (!stopFinished && nowMs - stageStartMs < STOP_TIMEOUT_MS) return Action.WAIT;
                stopTimedOut = !stopFinished;
                // Releasing the output is itself a display change; count the quiet time from here.
                enter(Stage.DISPLAY_SETTLE, nowMs);
                return Action.RELEASE_OUTPUT;
            case DISPLAY_SETTLE:
                if (!settled(nowMs, DISPLAY_QUIET_MS, DISPLAY_SETTLE_MAX_MS)) return Action.WAIT;
                enter(Stage.MODE_SETTLE, nowMs);
                return Action.RESTORE_DISPLAY_MODE;
            case MODE_SETTLE:
                if (!settled(nowMs, MODE_QUIET_MS, MODE_SETTLE_MAX_MS)) return Action.WAIT;
                stage = Stage.DONE;
                return Action.FINISH;
            default:
                return Action.WAIT;
        }
    }

    private void enter(Stage next, long nowMs) {
        stage = next;
        stageStartMs = nowMs;
        lastDisplayChangeMs = nowMs;
    }

    private boolean settled(long nowMs, long quietMs, long maxMs) {
        return nowMs - lastDisplayChangeMs >= quietMs || nowMs - stageStartMs >= maxMs;
    }
}

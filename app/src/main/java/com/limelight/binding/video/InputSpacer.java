package com.limelight.binding.video;

/**
 * Erratum 23 with Android's low-latency mode on: says how long a frame should wait so it doesn't reach
 * the decoder right behind the previous one. A frame on time waits nothing.
 *
 * Half a frame interval: frames on time are a whole interval apart, so one up to half an interval early
 * still waits nothing, and a backlog drains at twice the stream's rate. In direct submit the wait blocks
 * packet receive, as a full decoder's input also does, and the socket buffer has to take what arrives
 * meanwhile. How long a full input blocks in this mode and how much buffer the system grants on this TV
 * are not known, so the first report from it has to judge the hold: it carries the time held and the
 * granted buffer. Only the thread that submits decode units uses this.
 */
final class InputSpacer {
    // The longest wait at any frame rate, so a low one can't hold the receive thread or a stop for long.
    static final long MAX_HOLD_NS = 20_000_000;

    private boolean hasInput;
    private long lastInputNs;

    /** How long the frame about to go in should wait; an unknown frame rate waits nothing. */
    long holdNs(long nowNs, int fpsMilliHz) {
        if (!hasInput || fpsMilliHz < 1000) return 0;
        long spacingNs = Math.min(FramePacingPolicy.frameIntervalNs(fpsMilliHz) / 2, MAX_HOLD_NS);
        return Math.max(0, lastInputNs + spacingNs - nowNs);
    }

    /** A frame goes into the decoder now. */
    void onInput(long nowNs) {
        hasInput = true;
        lastInputNs = nowNs;
    }
}

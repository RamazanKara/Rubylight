package com.limelight.console;

/**
 * The plan of waking a PC to play on it, as a clock-driven decision: send a wake packet at once, send it again every
 * {@link #RESEND_MS} while the PC stays away, and give up after {@link #TIMEOUT_MS}. {@link HostWaker} asks it on every
 * tick; it holds no thread and no clock of its own, so the timing is tested with plain numbers.
 */
final class WakeLaunch {
    /** What to do on this tick. */
    enum Step {
        /** Nothing yet: the packet went out lately. */
        WAIT,
        /** Send a wake packet now. */
        SEND,
        /** The PC answers; carry on with the launch. */
        ONLINE,
        /** The PC did not wake in time. */
        TIMED_OUT
    }

    /** A resume from sleep takes well under this on the hardware we know of; longer counts as a failed wake. */
    static final long TIMEOUT_MS = 90_000;
    static final long RESEND_MS = 8_000;

    private final long startMs;
    private final long timeoutMs;
    private final long resendMs;
    private boolean sent;
    private long lastSentMs;

    WakeLaunch(long startMs) {
        this(startMs, TIMEOUT_MS, RESEND_MS);
    }

    WakeLaunch(long startMs, long timeoutMs, long resendMs) {
        this.startMs = startMs;
        this.timeoutMs = timeoutMs;
        this.resendMs = resendMs;
    }

    /** An answering PC wins over everything, including the time that ran out in the same tick. */
    Step next(boolean online, long nowMs) {
        if (online) {
            return Step.ONLINE;
        }
        if (nowMs - startMs >= timeoutMs) {
            return Step.TIMED_OUT;
        }
        if (!sent || nowMs - lastSentMs >= resendMs) {
            sent = true;
            lastSentMs = nowMs;
            return Step.SEND;
        }
        return Step.WAIT;
    }
}

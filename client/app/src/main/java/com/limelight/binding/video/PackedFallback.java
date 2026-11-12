package com.limelight.binding.video;

/**
 * What happens when packed full-chroma reconstruction stops working during a stream. The PC does
 * the packing, so presentation cannot simply continue without it: the stream is reconnected with
 * the setting off for the rest of that session (ordinary 4:2:0 video, a fresh negotiation), once.
 * The user's setting is not touched; the next session tries full chroma again.
 */
public final class PackedFallback {
    private PackedFallback() {}

    public enum Action {
        /** Reconnect without packed 4:4:4. */
        RECONNECT_WITHOUT_PACKED,
        /** End the stream: nothing left to turn off, or no way to reconnect now. */
        END_STREAM
    }

    /**
     * @param sessionUnpacked this session was already reconnected without packed 4:4:4, so a second
     *                        failure is not about it and degrading again would loop
     * @param canReconnect    the activity can start a replacement (foreground, a game to resume)
     */
    public static Action onFailure(boolean sessionUnpacked, boolean canReconnect) {
        return !sessionUnpacked && canReconnect ? Action.RECONNECT_WITHOUT_PACKED : Action.END_STREAM;
    }

    /** The setting as this session uses it: the saved setting, unless the session turned it off. */
    public static boolean effective(boolean setting, boolean sessionUnpacked) {
        return setting && !sessionUnpacked;
    }

    /**
     * Whether a start refused full chroma in the handshake is made again as it was, without it: when
     * repeating the launch resumes the game it started. A start that launches beside the PC's game
     * (a control action, Microphone only) would run again, so it reconnects to the running game
     * instead.
     */
    public static boolean restartsAsTheSameStart(boolean launchesBesideRunningGame) {
        return !launchesBesideRunningGame;
    }

    /** Short on purpose: the reason goes to the log, a long toast is cut off. */
    public static int toast() {
        return com.limelight.R.string.sharp_color_stopped;
    }

    /** The stage that failed when this device found, before the stream started, that it cannot show full chroma. */
    public static final String CHECK_STAGE = "full chroma check";

    /**
     * Runs {@code check} on a thread of its own and waits at most {@code timeoutMs} for it: a
     * decoder or GPU driver that never returns from the check must not hold the launch. No answer
     * in time counts as false, and the thread is left to end by itself.
     */
    public static boolean within(java.util.function.BooleanSupplier check, long timeoutMs, String what) {
        final boolean[] result = new boolean[1];
        Thread worker = new Thread(() -> {
            boolean value = check.getAsBoolean();
            synchronized (result) {
                result[0] = value;
            }
        }, "Full chroma check");
        worker.setDaemon(true);
        worker.start();
        try {
            worker.join(timeoutMs);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            return false;
        }
        if (worker.isAlive()) {
            com.limelight.LimeLog.warning("Full chroma check gave no answer within " + timeoutMs + " ms: " + what);
            return false;
        }
        synchronized (result) {
            return result[0];
        }
    }

    /**
     * Full chroma was refused at stream start, at {@code stage}: by the PC (or no mode is in common),
     * or by this device's own check ({@link #CHECK_STAGE}). The reason goes to the log.
     */
    public static int refusedToast(String stage) {
        return CHECK_STAGE.equals(stage) ? com.limelight.R.string.sharp_color_unavailable_device
                : com.limelight.R.string.sharp_color_unavailable_pc;
    }

    /** How long one mode's check may take before it counts as failed. Its own waits add up to about six seconds. */
    public static final long CHECK_TIMEOUT_MS = 10_000;
}

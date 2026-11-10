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

    /** Short on purpose: the reason goes to the log, a long toast is cut off. */
    public static String toast() {
        return "Full chroma stopped. Reconnecting without it.";
    }

    /** The stage that failed when this device found, before the stream started, that it cannot show full chroma. */
    public static final String CHECK_STAGE = "full chroma check";

    /** The stage that failed when the PC is known, before the launch, not to stream full chroma to this device. */
    public static final String HOST_STAGE = "full chroma offer";

    /** Why a stream cannot use full chroma, known before anything is launched on the PC. */
    public enum Unavailable {
        /** Only the New streaming engine carries it. */
        ENGINE(CHECK_STAGE),
        /** The PC says it offers no packed mode, or refused the last stream that asked and has not said otherwise. */
        HOST(HOST_STAGE),
        /** No decoder path on this device passed the check for this size and rate. */
        DEVICE(CHECK_STAGE),
        /** The PC and this device each have modes, none of them the same. */
        NO_COMMON_MODE(HOST_STAGE);

        /** What the failed stage is called, which picks {@link #refusedToast}'s wording. */
        public final String stage;

        Unavailable(String stage) {
            this.stage = stage;
        }
    }

    /**
     * Decided before the launch, so a stream that cannot have full chroma starts without it instead
     * of launching, being refused in the handshake and reconnecting.
     *
     * @param hostModes     the PC's packed modes: its serverinfo, else its last DESCRIBE, else -1
     * @param refusedBefore the PC does not say its modes and refused the last stream that asked
     * @param deviceModes   the modes this device verified; asked only when the PC does not rule it
     *                      out, since verifying costs a decode
     * @return why not, or null: the stream asks for full chroma
     */
    public static Unavailable beforeLaunch(boolean rustEngine, int hostModes, boolean refusedBefore,
                                           java.util.function.IntSupplier deviceModes) {
        if (!rustEngine) return Unavailable.ENGINE;
        if (hostModes == 0 || (hostModes < 0 && refusedBefore)) return Unavailable.HOST;
        int device = deviceModes.getAsInt();
        if (device == 0) return Unavailable.DEVICE;
        if (hostModes > 0 && (hostModes & device) == 0) return Unavailable.NO_COMMON_MODE;
        return null;
    }

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
    public static String refusedToast(String stage) {
        return CHECK_STAGE.equals(stage) ? "Full chroma isn't available for this stream. Starting without it."
                : "PC can't stream full chroma. Starting without it.";
    }

    /** How long one mode's check may take before it counts as failed. Its own waits add up to about six seconds. */
    public static final long CHECK_TIMEOUT_MS = 10_000;
}

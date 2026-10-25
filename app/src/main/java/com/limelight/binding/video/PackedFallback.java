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
}

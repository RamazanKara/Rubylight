package com.limelight;

/** When selecting a game asks, and what a stream that moved to another device may do. */
public final class ContinueHerePolicy {
    private ContinueHerePolicy() {}

    /** Selecting a game asks when another device plays this game, the PC offers the move and this is a plain start. */
    public static boolean asksOnSelect(boolean offered, boolean plainStart, boolean sameGame) {
        return offered && plainStart && sameGame;
    }

    /**
     * A stream that ended asks the PC if it moved, unless this device left on purpose or the PC cannot say. One that
     * goes on to reconnect does not wait for the answer on a link that may be dead: its resume asks once the PC answered.
     */
    public static boolean readsMoveAfterEnd(boolean offered, boolean leftOnPurpose, boolean recoveryFollows) {
        return offered && !leftOnPurpose && !recoveryFollows;
    }

    /**
     * A prompt saying the stream moved turns into "the game came back" when the PC no longer says it moved and this
     * device owns the game again. A note that ran out with the game elsewhere, or a game that ended, offers nothing.
     */
    public static boolean cameBack(boolean moved, boolean owns) {
        return !moved && owns;
    }

    /**
     * Whether a resume joins the game the PC runs, once the PC answered who plays: {@code /resume} joins it whoever owns it,
     * so only an answer that says it is another device's may. {@code too}: the user already chose to stream beside it.
     * Otherwise another device's same game asks first, and only a press that failed with nobody to ask about joins.
     */
    public static boolean joinsAfterRead(boolean too, boolean sameGame, boolean pressFailed) {
        return too ? sameGame || pressFailed : !sameGame && pressFailed;
    }

    /** {@link #resumeRefusal}: the game the user chose is not what runs any more. */
    public static final String SESSION_CHANGED = "session_changed";
    /** {@link #resumeRefusal}: another device's browser stream would end, and a join promises to stream beside it. */
    public static final String BROWSER_STREAM = "browser_stream";

    /**
     * Reads a refused {@code /resume} by the host's words, as {@code LaunchConfirmation} reads its prompts: a host without
     * these words keeps its own message. {@code join}: no game running is what this start could not check itself; any
     * other resume keeps the host's message and its recovery.
     */
    public static String resumeRefusal(boolean join, int code, String message) {
        String said = message == null ? "" : message.trim();
        if (code == 409 && said.equals("The game running on this PC changed")) return SESSION_CHANGED;
        if (join && code == 503 && said.equals("no application or remote monitor to resume")) return SESSION_CHANGED;
        if (code == 409 && said.equals("Another device is streaming over WebRTC")) return BROWSER_STREAM;
        return null;
    }

    /** A stream that moved never reconnects on its own: two devices must not fight over the stream. */
    public static boolean recovers(boolean transientFailure, boolean moved) {
        return transientFailure && !moved;
    }
}

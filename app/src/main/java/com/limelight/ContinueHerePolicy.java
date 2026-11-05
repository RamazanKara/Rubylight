package com.limelight;

/** When selecting a game asks, and what a stream that moved to another device may do. */
public final class ContinueHerePolicy {
    private ContinueHerePolicy() {}

    /** Selecting a game asks when another device plays this game, the PC offers the move and this is a plain start. */
    public static boolean asksOnSelect(boolean offered, boolean plainStart, boolean sameGame) {
        return offered && plainStart && sameGame;
    }

    /** A stream that ended asks the PC if it moved, unless this device left on purpose or the PC cannot say. */
    public static boolean readsMoveAfterEnd(boolean offered, boolean leftOnPurpose) {
        return offered && !leftOnPurpose;
    }

    /** A stream that moved never reconnects on its own: two devices must not fight over the stream. */
    public static boolean recovers(boolean transientFailure, boolean moved) {
        return transientFailure && !moved;
    }
}

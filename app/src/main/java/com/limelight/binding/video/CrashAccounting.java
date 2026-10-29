package com.limelight.binding.video;

import android.content.SharedPreferences;

/**
 * Whether a decoder failure is a decoder crash. The count behind the "video decoder crashed"
 * dialog disables features and resets settings, so it must not grow when the failure is only the
 * stream ending or (Experimental, Android TV) the TV dropping the signal: that is a display loss,
 * not a decoder fault.
 *
 * <p>A recent display change alone is no sign of a display loss: Nightjar's own refresh-rate and
 * display-mode requests change the display at stream start and again with content-rate votes, and
 * a real decoder failure shortly after those must still count.
 */
public final class CrashAccounting {
    public enum Verdict {
        /** A real decoder crash: counted as always. */
        COUNT,
        /** The stream had started ending (terminated, quit requested, stop begun). */
        STREAM_ENDING,
        /** Android TV, and the video display is off, invalid or gone. */
        DISPLAY_LOST
    }

    private CrashAccounting() {}

    /**
     * @param displayOff the video display is not on (off, invalid or removed) right now
     */
    public static Verdict classify(boolean streamEnding, boolean androidTv, boolean displayOff) {
        if (streamEnding) return Verdict.STREAM_ENDING;
        if (androidTv && displayOff) return Verdict.DISPLAY_LOST;
        return Verdict.COUNT;
    }

    /**
     * Counts a decoder the renderer gave up on, in the "DecoderTombstone" preferences. The stream
     * ended with its own dialog (-107), so the count is marked as told: the list shows no "Video
     * decoder crashed" after it and the settings reset of every third one (part of that dialog)
     * does not run. The count still makes the next stream run without reference frame invalidation.
     * Returns the new count.
     */
    public static int recordGiveUp(SharedPreferences tombstone, String hostUuid) {
        int count = tombstone.getInt("CrashCount", 0) + 1;
        // commit: the app may be gone right after this
        tombstone.edit()
                .putInt("CrashCount", count)
                .putInt("LastNotifiedCrashCount", count)
                .putString("CrashHostUuid", hostUuid)
                .commit();
        return count;
    }
}

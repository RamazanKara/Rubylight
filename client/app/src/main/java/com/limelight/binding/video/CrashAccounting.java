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

    private static final String KEY_CRASH_COUNT = "CrashCount";
    private static final String KEY_LAST_NOTIFIED_CRASH_COUNT = "LastNotifiedCrashCount";
    private static final String KEY_CRASH_HOST_UUID = "CrashHostUuid";
    private static final String KEY_STREAM_ACTIVE = "StreamActive";
    private static final String KEY_ACTIVE_PID = "ActivePid";
    private static final String KEY_ACTIVE_HOST_UUID = "ActiveHostUuid";

    /**
     * Marks a video stream active in the "DecoderTombstone" preferences so an uncaught/native crash
     * that kills the process before clean stop can be promoted on the next launch.
     */
    public static void markStreamActive(SharedPreferences tombstone, String hostUuid, int pid) {
        if (tombstone == null) return;
        synchronized (tombstone) {
            SharedPreferences.Editor editor = tombstone.edit()
                    .putBoolean(KEY_STREAM_ACTIVE, true)
                    .putInt(KEY_ACTIVE_PID, pid);
            if (hostUuid != null) {
                editor.putString(KEY_ACTIVE_HOST_UUID, hostUuid);
            } else {
                editor.remove(KEY_ACTIVE_HOST_UUID);
            }
            editor.commit();
        }
    }

    /** Clears the active-stream crash marker on clean stream stop or activity teardown. */
    public static void clearStreamActive(SharedPreferences tombstone) {
        if (tombstone == null) return;
        synchronized (tombstone) {
            if (!tombstone.getBoolean(KEY_STREAM_ACTIVE, false)) return;
            tombstone.edit()
                    .remove(KEY_STREAM_ACTIVE)
                    .remove(KEY_ACTIVE_PID)
                    .remove(KEY_ACTIVE_HOST_UUID)
                    .commit();
        }
    }

    /**
     * If a previous process exited mid-stream without clearing {@code StreamActive}, promotes it
     * into {@code CrashCount} without advancing {@code LastNotifiedCrashCount} so the user sees the
     * crash dialog and triggers the 3-strike safety reset on the third consecutive crash.
     */
    public static boolean promoteUncleanStreamExit(SharedPreferences tombstone, int currentPid) {
        if (tombstone == null) return false;
        synchronized (tombstone) {
            if (!tombstone.getBoolean(KEY_STREAM_ACTIVE, false)) return false;
            int activePid = tombstone.getInt(KEY_ACTIVE_PID, 0);
            if (activePid == 0 || activePid == currentPid) return false;
            int count = tombstone.getInt(KEY_CRASH_COUNT, 0) + 1;
            String hostUuid = tombstone.getString(KEY_ACTIVE_HOST_UUID, null);
            SharedPreferences.Editor editor = tombstone.edit()
                    .putInt(KEY_CRASH_COUNT, count)
                    .remove(KEY_STREAM_ACTIVE)
                    .remove(KEY_ACTIVE_PID)
                    .remove(KEY_ACTIVE_HOST_UUID);
            if (hostUuid != null) {
                editor.putString(KEY_CRASH_HOST_UUID, hostUuid);
            }
            editor.commit();
            return true;
        }
    }

    /**
     * Counts a decoder the renderer gave up on, in the "DecoderTombstone" preferences. Strikes 1 and
     * 2 mark {@code LastNotifiedCrashCount} equal to {@code CrashCount} so the launcher does not show
     * a duplicate dialog after -107 while still disabling RFI on the next try. Every third strike
     * leaves {@code LastNotifiedCrashCount < CrashCount} so {@code UiHelper.showDecoderCrashDialog}
     * runs the 3-strike streaming settings safety reset and notifies the user.
     */
    public static int recordGiveUp(SharedPreferences tombstone, String hostUuid) {
        synchronized (tombstone) {
            int count = tombstone.getInt(KEY_CRASH_COUNT, 0) + 1;
            int lastNotified = count % 3 == 0 ? count - 1 : count;
            // commit: the app may be gone right after this
            SharedPreferences.Editor editor = tombstone.edit()
                    .putInt(KEY_CRASH_COUNT, count)
                    .putInt(KEY_LAST_NOTIFIED_CRASH_COUNT, lastNotified)
                    .remove(KEY_STREAM_ACTIVE)
                    .remove(KEY_ACTIVE_PID)
                    .remove(KEY_ACTIVE_HOST_UUID);
            if (hostUuid != null) {
                editor.putString(KEY_CRASH_HOST_UUID, hostUuid);
            } else {
                editor.remove(KEY_CRASH_HOST_UUID);
            }
            editor.commit();
            return count;
        }
    }
}

package com.limelight.utils;

import android.content.Intent;

/**
 * Where a stream start came from: the screen and action, the shortcut or frontend, or the stream's own
 * reconnect. The stream logs it, so a log shows what asked the host to launch, or replace, a game.
 */
public final class LaunchSource {
    /** The stream intent's extra. Not a setting: it lives only as long as the intent. */
    public static final String EXTRA = "LaunchSource";

    public static final String LIBRARY_TAP = "library tap";
    public static final String LIBRARY_MENU = "library menu";
    public static final String LIBRARY_REPLACE = "library replace";
    public static final String LIBRARY_INPUT_ONLY = "library input only";
    public static final String PC_RESUME = "pc list resume";
    public static final String CONSOLE_RESUME = "console resume";
    public static final String CONSOLE_PLAY = "console game details";
    public static final String SHORTCUT = "shortcut";
    public static final String FRONTEND = "frontend";
    public static final String SHORTCUT_RUNNING = "pc shortcut, running game";
    public static final String RECONNECT = "reconnect";
    public static final String RETURN = "return to stream";
    public static final String RECOVERY = "recovery";
    /** A stream intent nothing labelled: another app's, or a start this list misses. */
    public static final String UNKNOWN = "unknown";

    private LaunchSource() { }

    /** Labels {@code intent} with {@code source}; returns it. */
    public static Intent put(Intent intent, String source) {
        return intent.putExtra(EXTRA, source);
    }

    /** The source an intent carries, {@link #UNKNOWN} without one. */
    public static String of(Intent intent) {
        String source = intent != null ? intent.getStringExtra(EXTRA) : null;
        return source != null && !source.isEmpty() ? source : UNKNOWN;
    }

    /** A shortcut or frontend start, with the app that sent it when Android says. */
    public static String external(boolean frontend, String referrer) {
        String source = frontend ? FRONTEND : SHORTCUT;
        return referrer != null && !referrer.isEmpty() ? source + " from " + referrer : source;
    }

    /** The stream starting again by itself, {@code why}, keeping the start that opened it. */
    public static String again(String why, Intent first) {
        String original = of(first);
        // A second reconnect names the first start, not a chain of reconnects.
        int after = original.indexOf(" after ");
        if (after >= 0) original = original.substring(after + " after ".length());
        return why + " after " + original;
    }

    /** The stream log's line. */
    public static String describe(Intent intent) {
        return "Stream launch from " + of(intent);
    }
}

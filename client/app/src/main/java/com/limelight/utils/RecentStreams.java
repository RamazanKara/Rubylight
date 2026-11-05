package com.limelight.utils;

import androidx.annotation.VisibleForTesting;

import java.util.ArrayDeque;

/**
 * This app process's last streams for the problem report, one line each: the bounded event log can
 * lose a session's start, and a report often comes several streams after the one that went wrong.
 * Rows hold counters and the device's own decoder names, never addresses or PC names.
 */
public final class RecentStreams {
    static final int MAX_ROWS = 8;

    private static final ArrayDeque<String> ROWS = new ArrayDeque<>();
    private static String link = "link unknown";

    private RecentStreams() {}

    /** The route of the stream that is connecting, for its row. */
    public static synchronized void noteLink(WiredLink.Hints hints) {
        link = hints.wireless ? "link wireless" : hints.mbps > 0 ? "link wired " + hints.mbps + " Mbps" : "link unknown";
    }

    /** Adds a finished stream's row, with the link noted for it. */
    public static synchronized void add(String row) {
        if (ROWS.size() == MAX_ROWS) {
            ROWS.removeFirst();
        }
        ROWS.addLast(row + " | " + link);
        link = "link unknown";
    }

    /** The report's section, ending with an empty line. */
    public static synchronized String section() {
        return "Recent streams, oldest first (up to " + MAX_ROWS + ", times in UTC):\n" +
                (ROWS.isEmpty() ? "None finished in this app process." : String.join("\n", ROWS)) + "\n\n";
    }

    @VisibleForTesting
    public static synchronized void clear() {
        ROWS.clear();
        link = "link unknown";
    }
}

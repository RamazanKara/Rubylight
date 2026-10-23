package com.limelight.computers;

import android.content.Context;
import android.content.SharedPreferences;

/**
 * The names people give their PCs, kept on this device only. A PC's own name keeps updating from
 * its polls; the alias is shown instead of it while one is set. Removing a PC forgets its alias.
 */
public final class HostNames {
    /** The longest alias, in characters the way people count them. */
    public static final int MAX_LENGTH = 40;

    static final String PREFERENCES = "HostNames";

    private HostNames() {
    }

    private static SharedPreferences preferences(Context context) {
        return context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE);
    }

    /** The alias for this PC, or null when it shows its own name. */
    public static String get(Context context, String hostUuid) {
        if (context == null || hostUuid == null) {
            return null;
        }
        String alias = preferences(context).getString(hostUuid, null);
        return alias == null || alias.isEmpty() ? null : alias;
    }

    /**
     * What to store for text typed in the rename dialog: trimmed and cut to {@link #MAX_LENGTH}, or null
     * when it is empty or just the PC's own name, which both mean "use the PC's own name".
     */
    public static String normalize(String typed, String ownName) {
        if (typed == null) {
            return null;
        }
        String alias = typed.trim();
        if (alias.codePointCount(0, alias.length()) > MAX_LENGTH) {
            alias = alias.substring(0, alias.offsetByCodePoints(0, MAX_LENGTH)).trim();
        }
        if (alias.isEmpty() || (ownName != null && alias.equals(ownName.trim()))) {
            return null;
        }
        return alias;
    }

    /** Stores the alias, or drops it when {@code alias} is null. */
    public static synchronized void set(Context context, String hostUuid, String alias) {
        if (context == null || hostUuid == null) {
            return;
        }
        SharedPreferences.Editor editor = preferences(context).edit();
        if (alias == null || alias.isEmpty()) {
            editor.remove(hostUuid);
        } else {
            editor.putString(hostUuid, alias);
        }
        editor.apply();
    }

    public static void forget(Context context, String hostUuid) {
        set(context, hostUuid, null);
    }
}

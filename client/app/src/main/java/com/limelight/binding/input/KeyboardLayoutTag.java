package com.limelight.binding.input;

/**
 * Picks the keyboard layout the client reports to the host (0x553E) as a BCP 47 tag, so the host
 * types the keys the client did not normalize in that layout. The first known source wins: the
 * IME's hint for physical keyboards, then the IME's own language. Nothing known means no report:
 * the host keeps its own layout rather than a guess.
 */
public final class KeyboardLayoutTag {
    // The wire limit (protocol/core/src/keyboard_layout.rs).
    static final int MAX_LENGTH = 64;

    private KeyboardLayoutTag() {}

    /** The tag to report, or null. Locale strings such as "de_DE" become "de-DE". */
    public static String pick(String... candidates) {
        for (String candidate : candidates) {
            String tag = normalize(candidate);
            if (tag != null) return tag;
        }
        return null;
    }

    static String normalize(String candidate) {
        if (candidate == null) return null;
        String tag = candidate.trim().replace('_', '-');
        // "und" is Android's "no language", not a layout.
        if (tag.isEmpty() || tag.length() > MAX_LENGTH || tag.equalsIgnoreCase("und")) return null;
        for (int i = 0; i < tag.length(); i++) {
            char c = tag.charAt(i);
            if (!(c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' || c == '-')) {
                return null;
            }
        }
        return tag;
    }
}

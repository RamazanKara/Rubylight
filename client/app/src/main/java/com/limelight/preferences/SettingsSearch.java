package com.limelight.preferences;

import java.text.Normalizer;
import java.util.Locale;

final class SettingsSearch {
    static boolean matches(String query, CharSequence title, CharSequence summary, CharSequence category) {
        String text = normalize((title == null ? "" : title) + " " +
                (summary == null ? "" : summary) + " " + (category == null ? "" : category));
        for (String word : normalize(query).trim().split("\\s+")) {
            if (!text.contains(word)) return false;
        }
        return true;
    }

    private static String normalize(String value) {
        return Normalizer.normalize(value, Normalizer.Form.NFKD)
                .toLowerCase(Locale.ROOT).replaceAll("\\p{M}+", "");
    }
}

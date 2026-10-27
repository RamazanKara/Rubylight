package com.limelight.preferences;

import org.junit.Test;
import java.util.Locale;
import static org.junit.Assert.*;

public class SettingsSearchTest {
    @Test
    public void wordsCanMatchTitleDescriptionAndCategoryInAnyOrder() {
        assertTrue(SettingsSearch.matches("  decode   VIDEO ", "Low latency", "Faster decode", "Video"));
        assertFalse(SettingsSearch.matches("video bitrate", "Low latency", "Faster decode", "Video"));
        assertTrue(SettingsSearch.matches("", "Low latency", null, null));
    }

    @Test
    public void localizedTextIsAccentInsensitiveAndDoesNotInterpretRegex() {
        Locale old = Locale.getDefault();
        try {
            Locale.setDefault(new Locale("tr"));
            assertTrue(SettingsSearch.matches("input", "INPUT", null, null));
            assertTrue(SettingsSearch.matches("ecran", "Écran", null, null));
            assertFalse(SettingsSearch.matches(".*", "Everything", null, null));
            assertTrue(SettingsSearch.matches("4:4:4", "YUV 4:4:4", null, null));
        } finally {
            Locale.setDefault(old);
        }
    }
}

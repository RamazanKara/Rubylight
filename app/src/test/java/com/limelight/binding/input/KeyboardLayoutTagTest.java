package com.limelight.binding.input;

import org.junit.Test;

import static org.junit.Assert.*;

public class KeyboardLayoutTagTest {
    @Test
    public void theFirstKnownSourceWins() {
        assertEquals("the IME's physical keyboard hint first", "de-CH", KeyboardLayoutTag.pick("de-CH", "en-US"));
        assertEquals("then the IME's language", "en-US", KeyboardLayoutTag.pick("", "en-US", "de_DE"));
        assertEquals("then its locale", "de-DE", KeyboardLayoutTag.pick(null, "", "de_DE"));
    }

    @Test
    public void nothingKnownReportsNothing() {
        assertNull(KeyboardLayoutTag.pick());
        assertNull(KeyboardLayoutTag.pick(null, "", "  ", "und"));
    }

    @Test
    public void onlyTagsTheWireCarriesAreReported() {
        String longest = "a" + "-abcdefgh".repeat(7);
        assertEquals(64, longest.length());
        assertEquals(longest, KeyboardLayoutTag.pick(longest));
        assertNull(KeyboardLayoutTag.pick(longest + "x"));
        assertNull(KeyboardLayoutTag.pick("de DE"));
        assertNull(KeyboardLayoutTag.pick("dé-DE"));
        assertEquals("a bad source falls through", "ja-JP", KeyboardLayoutTag.pick("de DE", "ja-JP"));
        assertEquals("sr-Latn-RS", KeyboardLayoutTag.pick(" sr_Latn_RS "));
    }
}

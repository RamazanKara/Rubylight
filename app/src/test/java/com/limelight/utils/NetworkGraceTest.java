package com.limelight.utils;

import org.junit.Test;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

public class NetworkGraceTest {
    @Test public void onlyOfferedChoicesAreAccepted() {
        assertEquals(0, NetworkGrace.parseSeconds(null));
        assertEquals(0, NetworkGrace.parseSeconds(""));
        assertEquals(0, NetworkGrace.parseSeconds("abc"));
        assertEquals(0, NetworkGrace.parseSeconds("0"));
        assertEquals(20, NetworkGrace.parseSeconds("20"));
        assertEquals(30, NetworkGrace.parseSeconds(" 30 "));
        assertEquals(60, NetworkGrace.parseSeconds("60"));
        // A hand-edited or future value must not stretch the timeout without bound.
        assertEquals(0, NetworkGrace.parseSeconds("3600"));
        assertEquals(0, NetworkGrace.parseSeconds("-20"));
        assertEquals(0, NetworkGrace.parseSeconds("15"));
    }

    @Test public void offAndShortValuesKeepTheNativeDefault() {
        assertFalse(NetworkGrace.enabled(0));
        assertFalse(NetworkGrace.enabled(NetworkGrace.DEFAULT_TIMEOUT_SECONDS));
        assertEquals(0, NetworkGrace.nativeMilliseconds(0));
        assertEquals(0, NetworkGrace.nativeMilliseconds(NetworkGrace.DEFAULT_TIMEOUT_SECONDS));
        assertEquals(0, NetworkGrace.nativeMilliseconds(-30));
    }

    @Test public void everyOfferedChoiceAboveOffIsLongerThanTheDefaultTimeout() {
        for (int seconds : NetworkGrace.CHOICES_SECONDS) {
            if (seconds == 0) continue;
            assertTrue(NetworkGrace.enabled(seconds));
            assertEquals(seconds * 1000, NetworkGrace.nativeMilliseconds(seconds));
        }
    }

    @Test public void onlyATransportLossWithTheSettingOnCanBeAGiveUp() {
        assertTrue(NetworkGrace.mayHaveGivenUp(30, -1));
        assertFalse(NetworkGrace.mayHaveGivenUp(0, -1));
        // Graceful exits, media timeouts and socket errors keep their own messages.
        for (int code : new int[] {0, -100, -101, -102, -103, -104, 101, 110, 10054}) {
            assertFalse("code " + code, NetworkGrace.mayHaveGivenUp(30, code));
        }
    }
}

package com.limelight.binding.input.touch;

import org.junit.Test;

import static org.junit.Assert.*;

public class TouchModeTest {
    @Test
    public void olderInstallsKeepTheirMode() {
        assertEquals(TouchMode.TRACKPAD, TouchMode.resolve(null, true, false));
        assertEquals(TouchMode.DIRECT, TouchMode.resolve(null, false, false));
        assertEquals(TouchMode.MULTI_TOUCH, TouchMode.resolve(null, false, true));
        // Older versions ignore the native touch switch while the trackpad switch is on.
        assertEquals(TouchMode.TRACKPAD, TouchMode.resolve(null, true, true));
        assertEquals(TouchMode.DIRECT, TouchMode.resolve("unknown", false, false));
    }

    @Test
    public void everyModeSurvivesARoundTrip() {
        for (int mode = TouchMode.TRACKPAD; mode <= TouchMode.OFF; mode++) {
            assertEquals(mode, TouchMode.resolve(TouchMode.value(mode), TouchMode.legacyTrackpad(mode),
                    TouchMode.legacyNativeTouch(mode)));
        }
    }

    @Test
    public void newerModesLookLikeTrackpadToOlderVersions() {
        assertTrue(TouchMode.legacyTrackpad(TouchMode.LOCAL_CURSOR));
        assertTrue(TouchMode.legacyTrackpad(TouchMode.OFF));
        assertFalse(TouchMode.legacyNativeTouch(TouchMode.LOCAL_CURSOR));
        assertFalse(TouchMode.legacyNativeTouch(TouchMode.OFF));
    }

    @Test
    public void aModePickedInAnOlderVersionWins() {
        // Local cursor was saved, then an older version switched to Direct mouse or Multi-touch.
        assertEquals(TouchMode.DIRECT, TouchMode.resolve("local_cursor", false, false));
        assertEquals(TouchMode.MULTI_TOUCH, TouchMode.resolve("off", false, true));
        // Direct mouse was saved, then an older version switched to Trackpad.
        assertEquals(TouchMode.TRACKPAD, TouchMode.resolve("direct", true, false));
        assertEquals(TouchMode.DIRECT, TouchMode.resolve("multi_touch", false, false));
        // A newer mode stays while older versions still read it as Trackpad.
        assertEquals(TouchMode.LOCAL_CURSOR, TouchMode.resolve("local_cursor", true, true));
    }

    @Test
    public void onlyTrackpadAndLocalCursorAreRelative() {
        assertTrue(TouchMode.isRelative(TouchMode.TRACKPAD));
        assertTrue(TouchMode.isRelative(TouchMode.LOCAL_CURSOR));
        assertFalse(TouchMode.isRelative(TouchMode.DIRECT));
        assertFalse(TouchMode.isRelative(TouchMode.MULTI_TOUCH));
        assertFalse(TouchMode.isRelative(TouchMode.OFF));
    }
}

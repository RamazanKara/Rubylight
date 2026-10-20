package com.limelight.nvstream.jni;

import org.junit.Test;

import static org.junit.Assert.*;

public class RustBridgeTest {
    private static final String SUNSHINE = "7.1.431.-1";

    @Test public void ordinaryStreamsUseTheRustEngine() {
        assertNull(RustBridge.fallbackReason(SUNSHINE, 0x0101, MoonBridge.VIDEO_FORMAT_H264 |
                MoonBridge.VIDEO_FORMAT_H265 | MoonBridge.VIDEO_FORMAT_AV1_MAIN8, false, false));
        assertNull(RustBridge.fallbackReason("7.1.450.0", 0x0001, MoonBridge.VIDEO_FORMAT_H264, false, false));
        // Input-only streams negotiate no codec.
        assertNull(RustBridge.fallbackReason(SUNSHINE, 0, MoonBridge.VIDEO_FORMAT_H264, true, false));
    }

    @Test public void unsupportedStreamsFallBackToTheCEngine() {
        assertNotNull(RustBridge.fallbackReason(SUNSHINE, 0x0101, MoonBridge.VIDEO_FORMAT_PYROWAVE, false, false));
        assertNotNull(RustBridge.fallbackReason(SUNSHINE, 0x0101,
                MoonBridge.VIDEO_FORMAT_H264 | MoonBridge.VIDEO_FORMAT_PYROWAVE_MAIN10, false, false));
        assertNotNull(RustBridge.fallbackReason(SUNSHINE, 0x0101, MoonBridge.VIDEO_FORMAT_H264, false, true));
        assertNotNull(RustBridge.fallbackReason("7.1.430.0", 0x0101, MoonBridge.VIDEO_FORMAT_H264, false, false));
        assertNotNull(RustBridge.fallbackReason("5.0.0", 0x0101, MoonBridge.VIDEO_FORMAT_H264, false, false));
        assertNotNull(RustBridge.fallbackReason(null, 0x0101, MoonBridge.VIDEO_FORMAT_H264, false, false));
        assertNotNull(RustBridge.fallbackReason("garbage", 0x0101, MoonBridge.VIDEO_FORMAT_H264, false, false));
        assertNotNull(RustBridge.fallbackReason(SUNSHINE, 0, MoonBridge.VIDEO_FORMAT_H264, false, false));
    }

    @Test public void appVersionsCompareNumerically() {
        int[] min = {7, 1, 431};
        assertTrue(RustBridge.appVersionAtLeast("7.1.431", min));
        assertTrue(RustBridge.appVersionAtLeast("7.2", min));
        assertTrue(RustBridge.appVersionAtLeast("8.0.0.0", min));
        assertFalse(RustBridge.appVersionAtLeast("7.1", min));
        assertFalse(RustBridge.appVersionAtLeast("7.1.-1", min));
    }
}

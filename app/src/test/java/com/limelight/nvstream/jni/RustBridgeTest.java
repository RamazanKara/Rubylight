package com.limelight.nvstream.jni;

import org.junit.Test;

import static org.junit.Assert.*;

public class RustBridgeTest {
    private static final String SUNSHINE = "7.1.431.-1";

    @Test public void ordinaryStreamsUseTheRustEngine() {
        assertNull(RustBridge.fallbackReason(SUNSHINE, 0x0101, false));
        assertNull(RustBridge.fallbackReason("7.1.450.0", 0x0001, false));
        // Input-only streams negotiate no codec.
        assertNull(RustBridge.fallbackReason(SUNSHINE, 0, true));
    }

    @Test public void unsupportedStreamsFallBackToTheCEngine() {
        assertNotNull(RustBridge.fallbackReason("7.1.430.0", 0x0101, false));
        assertNotNull(RustBridge.fallbackReason("5.0.0", 0x0101, false));
        assertNotNull(RustBridge.fallbackReason(null, 0x0101, false));
        assertNotNull(RustBridge.fallbackReason("garbage", 0x0101, false));
        assertNotNull(RustBridge.fallbackReason(SUNSHINE, 0, false));
    }

    @Test public void everyFallbackReasonNamesItsCause() {
        assertTrue(RustBridge.fallbackReason("7.1.430.0", 0x0101, false).contains("GFE 3.22"));
        assertTrue(RustBridge.fallbackReason("7.1.430.0", 0x0101, false).contains("7.1.430.0"));
        assertTrue(RustBridge.fallbackReason(SUNSHINE, 0, false).contains("codec"));
    }

    @Test public void engineSummaryNamesTheEngineAndTheFallbackReason() {
        StreamEngine.record(false, false, null);
        assertEquals("C (moonlight-common-c)", StreamEngine.summary());
        assertEquals("C", StreamEngine.label());
        assertFalse(StreamEngine.fellBack());

        StreamEngine.record(true, true, null);
        assertEquals("Rust (nightjar-core)", StreamEngine.summary());
        assertEquals("Rust", StreamEngine.label());
        assertTrue(StreamEngine.isRust());
        assertFalse(StreamEngine.fellBack());

        StreamEngine.record(true, false, "libnightjar_core.so did not load");
        String fallback = StreamEngine.summary();
        assertTrue(fallback.startsWith("C (moonlight-common-c)"));
        assertTrue(fallback.contains("Rust engine requested but unavailable"));
        assertTrue(fallback.contains("libnightjar_core.so did not load"));
        assertEquals("C (Rust fell back)", StreamEngine.label());
        assertTrue(StreamEngine.fellBack());
        StreamEngine.record(false, false, null);
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

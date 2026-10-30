package com.limelight.utils;

import org.junit.Test;

import java.util.HashMap;
import java.util.Locale;
import java.util.Map;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

/** The launch luminance report, as Moonlight V+ builds it (HdrBrightnessTest.kt). */
public class HdrBrightnessTest {
    private static final float[] NO_RATIO = {Float.NaN, -1f, 0f, 1f, Float.POSITIVE_INFINITY, Float.MAX_VALUE, 100f};

    @Test
    public void aCompleteSystemReportAlwaysWinsOverTheRatio() {
        for (float ratio : new float[] {Float.NaN, 5f, Float.POSITIVE_INFINITY, Float.MAX_VALUE}) {
            HdrBrightness report = HdrBrightness.fromDisplay(400.125f, 0f, 300.25f, ratio);
            assertNotNull(report);
            assertEquals(400.125f, report.maxNits, 0f);
            assertEquals(0f, report.minNits, 0f);
            assertEquals(300.25f, report.maxFullFrameNits, 0f);
            assertFalse(report.estimatedFromRatio);
        }
    }

    @Test
    public void aMissingPeakIsEstimatedFromTheRatio() {
        HdrBrightness report = HdrBrightness.fromDisplay(-1f, Float.NaN, -1f, 5f);
        assertEquals(1500f, report.maxNits, 0f);
        assertEquals(0.001f, report.minNits, 0f);
        assertEquals(300f, report.maxFullFrameNits, 0f);
        assertTrue(report.estimatedFromRatio);
        HdrBrightness fractional = HdrBrightness.fromDisplay(-1f, -1f, -1f, 3.25f);
        assertEquals(975f, fractional.maxNits, 0f);
        assertEquals(195f, fractional.maxFullFrameNits, 0f);
    }

    @Test
    public void anEstimateKeepsAKnownBlackLevelAndFullFrame() {
        HdrBrightness report = HdrBrightness.fromDisplay(-1f, 0.0004f, 600.125f, 5f);
        assertEquals(1500f, report.maxNits, 0f);
        assertEquals(0.0004f, report.minNits, 0f);
        assertEquals(600.125f, report.maxFullFrameNits, 0f);
        // A known full frame is a lower bound for the estimated peak.
        HdrBrightness bright = HdrBrightness.fromDisplay(-1f, 0f, 1200.25f, 3f);
        assertEquals(1200.25f, bright.maxNits, 0f);
        assertEquals(1200.25f, bright.maxFullFrameNits, 0f);
    }

    @Test
    public void withoutAUsableRatioUnknownCapabilitiesAreLeftToTheHost() {
        for (float ratio : NO_RATIO) {
            assertNull(HdrBrightness.fromDisplay(-1f, -1f, -1f, ratio));
        }
    }

    @Test
    public void aValidPeakNeverRepairsTheOtherFields() {
        for (float ratio : new float[] {Float.NaN, 5f, Float.POSITIVE_INFINITY}) {
            assertNull(HdrBrightness.fromDisplay(400.125f, -1f, 300.25f, ratio));
            assertNull(HdrBrightness.fromDisplay(400f, 0.001f, -1f, ratio));
            assertNull(HdrBrightness.fromDisplay(400f, 0.001f, 600f, ratio));
            assertNull(HdrBrightness.fromDisplay(1000f, 20f, 10f, ratio));
        }
    }

    @Test
    public void theHostsRangeAndOrderAreEnforcedWithoutClamping() {
        for (float invalid : new float[] {Float.NaN, Float.POSITIVE_INFINITY, Float.NEGATIVE_INFINITY, -1f}) {
            assertNull(HdrBrightness.create(invalid, 0.001f, 600f));
            assertNull(HdrBrightness.create(1000f, invalid, 600f));
            assertNull(HdrBrightness.create(1000f, 0.001f, invalid));
        }
        assertNull(HdrBrightness.create(0.5f, 0f, 0.5f));
        assertNull(HdrBrightness.create(10001f, 0f, 600f));
        assertNull(HdrBrightness.create(1000f, 101f, 600f));
        assertNull(HdrBrightness.create(400f, 0.001f, 600f));
        assertNull(HdrBrightness.create(1000f, 20f, 10f));
        assertNull(HdrBrightness.create(1000f, 0f, 0f));
        assertNotNull(HdrBrightness.create(10000f, 100f, 10000f));
        assertNotNull(HdrBrightness.create(1f, 0f, 1f));
    }

    @Test
    public void theBestRatioIsTheLargerUsableOne() {
        assertEquals(5f, HdrBrightness.bestHdrSdrRatio(2f, 5f), 0f);
        assertEquals(2f, HdrBrightness.bestHdrSdrRatio(2f, Float.NaN), 0f);
        assertEquals(3f, HdrBrightness.bestHdrSdrRatio(1f, 3f), 0f);
        assertTrue(Float.isNaN(HdrBrightness.bestHdrSdrRatio(1f, Float.NaN)));
    }

    @Test
    public void theQueryUsesTheProtocolNamesAndRoundTripsFloatsInAnyLocale() {
        Locale previous = Locale.getDefault();
        try {
            Locale.setDefault(Locale.GERMANY);
            HdrBrightness report = HdrBrightness.create(1000.125f, 0.0000004f, 600.25f);
            Map<String, String> fields = new HashMap<>();
            for (String pair : report.toQueryParameters().substring(1).split("&")) {
                fields.put(pair.substring(0, pair.indexOf('=')), pair.substring(pair.indexOf('=') + 1));
            }
            assertEquals(3, fields.size());
            assertEquals(report.minNits, Float.parseFloat(fields.get("minBrightness")), 0f);
            assertEquals(report.maxNits, Float.parseFloat(fields.get("maxBrightness")), 0f);
            assertEquals(report.maxFullFrameNits, Float.parseFloat(fields.get("maxAverageBrightness")), 0f);
        } finally {
            Locale.setDefault(previous);
        }
    }
}

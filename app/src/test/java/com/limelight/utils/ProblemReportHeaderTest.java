package com.limelight.utils;

import android.app.Application;

import com.limelight.LimeLog;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowBuild;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class)
public class ProblemReportHeaderTest {
    private static final long TIME = 1_793_962_338_750L; // 2026-11-06T...: only the format matters

    @Test public void theHeaderNamesTheDeviceAFixIsGatedOn() {
        ShadowBuild.setManufacturer("Amazon");
        ShadowBuild.setModel("AFTNE4E43F");
        ShadowBuild.setHardware("mt8696");
        String[] lines = ProblemReport.header().split("\n");
        assertEquals("Device: Amazon AFTNE4E43F, hardware mt8696", lines[1]);
    }

    @Test public void theHeaderSaysHowManyEntriesTheLogKeeps() {
        String header = ProblemReport.header();
        assertTrue(header, header.contains("up to " + RedactedLog.MAX_ENTRIES + " entries"));
        assertEquals(400, RedactedLog.MAX_ENTRIES);
        assertFalse(header, header.contains("200"));
        assertTrue(header, header.contains("times in UTC"));
    }

    @Test public void aClockStampsEntriesInUtcAndRedactionLeavesTheStampAlone() {
        RedactedLog log = new RedactedLog(() -> TIME);
        log.add("WARNING", "lost 192.168.1.9 at 10:20:30");
        String line = log.snapshot();
        assertTrue(line, line.matches("\\d{4}-\\d{2}-\\d{2}T\\d{2}:\\d{2}:\\d{2}Z WARNING lost \\[address\\] at .*"));
    }

    @Test public void levelsKeepTheirWordsBehindTheStamp() {
        RedactedLog log = new RedactedLog(() -> 0L);
        log.add("SEVERE", "broke");
        log.add("INFO", "Stream starting");
        assertEquals("1970-01-01T00:00:00Z ERROR broke\n1970-01-01T00:00:00Z INFO Stream starting", log.snapshot());
    }

    @Test public void withoutAClockEntriesHaveNoStamp() {
        RedactedLog log = new RedactedLog();
        log.add("INFO", "Stream starting");
        assertEquals("INFO Stream starting", log.snapshot());
    }

    @Test public void theAppLogStampsItsEntries() {
        LimeLog.warning("header test stamp line");
        String log = LimeLog.getRedactedLog();
        assertTrue(log, log.matches("(?s).*\\d{4}-\\d{2}-\\d{2}T\\d{2}:\\d{2}:\\d{2}Z WARNING header test stamp line.*"));
    }
}

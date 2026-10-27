package com.limelight.binding.video;

import org.junit.Test;

import static org.junit.Assert.*;

public class DisplayRefreshDiagnosticsTest {
    @Test public void identicalObservationsDoNotFloodTheReport() {
        DisplayRefreshDiagnostics diagnostics = new DisplayRefreshDiagnostics();
        assertEquals("Display refresh policy: api=36, display=0, mode=1, arrSupported=false, " +
                        "selected=max Hz, windowMode=0, windowHz=0.0, appSurfaceHz=120.0",
                diagnostics.update(36, 0, 1, false, true, false, 0, 0, 120f));
        for (int event = 0; event < 100; event++) {
            assertNull(diagnostics.update(36, 0, 1, false, true, false, 0, 0, 120f));
        }
    }

    @Test public void rawCapabilityChangesDoNotPretendTheSelectedPolicyChanged() {
        DisplayRefreshDiagnostics diagnostics = new DisplayRefreshDiagnostics();
        diagnostics.update(36, 0, 1, false, true, false, 0, 0, 120f);
        String promoted = diagnostics.update(36, 0, 1, true, true, false, 0, 0, 120f);
        assertTrue(promoted.contains("arrSupported=true, selected=max Hz"));
        assertNull(diagnostics.update(36, 0, 1, true, true, false, 0, 0, 120f));
        assertTrue(diagnostics.update(36, 0, 1, false, true, false, 0, 0, 120f)
                .contains("arrSupported=false"));
    }

    @Test public void modeAndTargetChangesAreReportedEvenWhenCapabilityAndVotesMatch() {
        DisplayRefreshDiagnostics diagnostics = new DisplayRefreshDiagnostics();
        diagnostics.update(36, 0, 1, true, true, true, 0, 0, 0f);
        assertTrue(diagnostics.update(36, 0, 2, true, true, true, 0, 0, 0f).contains("mode=2"));
        assertTrue(diagnostics.update(36, 3, 2, true, true, true, 0, 0, 0f).contains("display=3"));
    }

    @Test public void unavailableApisAreDistinctFromUnsupportedHardware() {
        DisplayRefreshDiagnostics diagnostics = new DisplayRefreshDiagnostics();
        String beforeApi36 = diagnostics.update(35, 0, 1, false, true, false, 0, 0, null);
        assertTrue(beforeApi36.contains("mode=1, arrSupported=unavailable"));
        assertTrue(beforeApi36.endsWith("appSurfaceHz=unavailable"));
        String legacy = diagnostics.update(21, 0, 0, false, false, false, 0, 60, null);
        assertTrue(legacy.contains("mode=unavailable, arrSupported=unavailable, selected=disabled"));
        assertTrue(legacy.contains("windowMode=unavailable, windowHz=60.0"));
    }

    @Test public void windowAndSuccessfulSurfaceVotesRemainSeparateFromRawSupport() {
        DisplayRefreshDiagnostics diagnostics = new DisplayRefreshDiagnostics();
        diagnostics.update(36, 0, 1, true, true, true, 2, 120, null);
        String clearedWindow = diagnostics.update(36, 0, 1, true, true, true, 0, 0, null);
        assertTrue(clearedWindow.contains("selected=ARR, windowMode=0, windowHz=0.0"));
        assertTrue(diagnostics.update(36, 0, 1, true, true, true, 0, 0, 0f).endsWith("appSurfaceHz=0.0"));
        assertTrue(diagnostics.update(36, 0, 1, true, false, false, 0, 0, 60f)
                .contains("arrSupported=true, selected=disabled"));
    }
}

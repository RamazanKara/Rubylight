package com.limelight.utils;

import org.junit.Test;
import static org.junit.Assert.*;

public class StreamHealthTest {
    private StreamHealth.Sample sample(long time, boolean fresh, float loss, float decode,
                                      float host, float display, float released, float shown, int heat) {
        return new StreamHealth.Sample(time, fresh, 60, loss, 1, decode, host, display,
                released, released, shown, heat, false);
    }
    @Test public void hostEvidenceSaysWhetherItIncludesTheSend() {
        StreamHealth withSend = new StreamHealth();
        StreamHealth withoutSend = new StreamHealth();
        for (int n = 1; n <= 3; n++) {
            withSend.add(new StreamHealth.Sample(n * 1000, true, 60, 0, 1, 4, 5, 2, 60, 60, 60, 0, true));
            withoutSend.add(new StreamHealth.Sample(n * 1000, true, 60, 0, 1, 4, 5, 2, 60, 60, 60, 0, false));
        }
        assertTrue(withSend.report(3000).hostIncludesSend);
        assertFalse(withoutSend.report(3000).hostIncludesSend);
        // After a switch from reports to the frame header's value the label follows the worst value shown.
        StreamHealth mixed = new StreamHealth();
        mixed.add(new StreamHealth.Sample(1000, true, 60, 0, 1, 4, 5, 2, 60, 60, 60, 0, true));
        mixed.add(new StreamHealth.Sample(2000, true, 60, 0, 1, 4, 9, 2, 60, 60, 60, 0, false));
        assertEquals(9f, mixed.report(2000).worstHostP95Ms, 0f);
        assertFalse(mixed.report(2000).hostIncludesSend);
        assertFalse(new StreamHealth().report(1000).hostIncludesSend);
    }
    @Test public void identifiesSimultaneousEvidenceWithoutInventingRootCause() {
        StreamHealth health = new StreamHealth();
        for (int n = 1; n <= 5; n++) health.add(sample(n * 1000, true, 4, 20, 25, 50, 60, 40, 2));
        StreamHealth.Report report = health.report(5000);
        assertEquals(5, report.findings.size());
        assertTrue(report.canSuggestBitrate());
        assertEquals(25f, report.worstHostP95Ms, 0f);
    }
    @Test public void presentationIsJudgedDecodedToOnScreenWhileTheReportShowsTheHandOffWait() {
        StreamHealth health = new StreamHealth();
        // Display wait ends at the hand-off (2 ms); decoded to on screen takes 50 ms with frames lost on screen.
        for (int n = 1; n <= 5; n++) health.add(new StreamHealth.Sample(n * 1000, true, 60, 0, 1, 4, 3, 2,
                60, 60, 40, 0, false, -1, 50));
        StreamHealth.Report report = health.report(5000);
        assertTrue(report.findings.contains(StreamHealth.Finding.PRESENTATION));
        assertEquals(2f, report.worstDisplayP95Ms, 0f);
        // Without a screen time the hand-off wait is all there is to judge.
        StreamHealth handOffOnly = new StreamHealth();
        for (int n = 1; n <= 5; n++) handOffOnly.add(new StreamHealth.Sample(n * 1000, true, 60, 0, 1, 4, 3, 2,
                60, 60, 40, 0, false, -1, -1));
        assertFalse(handOffOnly.report(5000).findings.contains(StreamHealth.Finding.PRESENTATION));
    }
    @Test public void staticVideoUnavailableCountersAndOneSpikeAreNotDiagnoses() {
        StreamHealth health = new StreamHealth();
        health.add(sample(1000, true, 3, 30, 40, 60, 60, 20, 2));
        for (int n = 2; n <= 6; n++) health.add(sample(n * 1000, true, -1, Float.NaN, -1, -1, 4, 4, 0));
        assertTrue(health.report(6000).findings.isEmpty());
        assertFalse(health.report(9000).canSuggestBitrate());
    }
    @Test public void staleDataExpiresAndDuplicateTicksDoNotIncreaseEvidence() {
        StreamHealth health = new StreamHealth();
        for (int n = 0; n < 100; n++) health.add(sample(1000, true, 4, 30, 40, 60, 60, 20, 2));
        assertEquals(1, health.report(1000).samples);
        assertEquals(0, health.report(32000).samples);
        assertFalse(health.report(32000).recent);
    }
    @Test public void absentFramesDoNotBecomeHealthyMeasurements() {
        StreamHealth health = new StreamHealth();
        for (int n = 1; n <= 6; n++) health.add(sample(n * 1000, false, 0, 0, 0, 0, 0, 0, 0));
        assertEquals(0, health.report(6000).samples);
        assertFalse(health.report(6000).recent);
    }
    @Test public void nonFiniteCountersCannotBecomeBottleneckEvidence() {
        StreamHealth health = new StreamHealth();
        for (int n = 1; n <= 4; n++) health.add(new StreamHealth.Sample(n * 1000, true, 60,
                Float.POSITIVE_INFINITY, Float.POSITIVE_INFINITY, Float.NaN, Float.POSITIVE_INFINITY,
                Float.POSITIVE_INFINITY, 60, Float.POSITIVE_INFINITY, 0, 0, false));
        assertTrue(health.report(4000).findings.isEmpty());
        assertEquals(-1, health.report(4000).worstHostP95Ms, 0);
    }

    // One healthy sample per second, with a three-second loss burst every two minutes.
    private static StreamHealth scanRhythm(int minutes) {
        StreamHealth health = new StreamHealth();
        for (int second = 1; second <= minutes * 60; second++) {
            boolean stall = second % 120 >= 30 && second % 120 < 33;
            health.add(new StreamHealth.Sample(second * 1000L, true, 60, stall ? 6 : 0, 1, 4, 3, 2,
                    60, 60, 60, 0, false));
        }
        return health;
    }
    @Test public void regularStallsBecomeARhythmFinding() {
        StreamHealth health = scanRhythm(10);
        StreamHealth.Report report = health.report(600_000);
        assertEquals(120_000, report.stallPeriodMs);
        assertEquals(5, report.stalls);
        assertEquals(120_000, health.stallPeriodMs());
        // The last 30 seconds were healthy; only the rhythm is reported.
        assertEquals(java.util.Collections.singletonList(StreamHealth.Finding.PERIODIC), report.findings);
    }
    @Test public void fewStallsAndContinuousLossAreNoRhythm() {
        assertEquals(0, scanRhythm(5).report(300_000).stallPeriodMs);
        StreamHealth health = new StreamHealth();
        for (int n = 1; n <= 900; n++) health.add(sample(n * 1000L, true, 4, 1, 1, 1, 60, 60, 0));
        StreamHealth.Report report = health.report(900_000);
        assertEquals(1, report.stalls);
        assertFalse(report.findings.contains(StreamHealth.Finding.PERIODIC));
    }
    @Test public void stallRhythmExpiresWithItsHistory() {
        StreamHealth health = scanRhythm(10);
        health.add(sample(600_000 + StreamHealth.STALL_HISTORY_MS, true, 0, 1, 1, 1, 60, 60, 0));
        StreamHealth.Report report = health.report(600_000 + StreamHealth.STALL_HISTORY_MS);
        assertEquals(0, report.stalls);
        assertEquals(0, report.stallPeriodMs);
        assertEquals(0, health.stallPeriodMs());
    }
    @Test public void staleOrUnmeasuredSamplesAreNotStalls() {
        StreamHealth health = new StreamHealth();
        for (int n = 1; n <= 600; n++) health.add(new StreamHealth.Sample(n * 1000L, n % 120 != 30, 60,
                n % 120 == 30 ? 50 : 0, 1, 1, 1, 1, 60, 60, 60, 0, false));
        assertEquals(0, health.report(600_000).stalls);
    }
}

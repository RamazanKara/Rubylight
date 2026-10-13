package com.limelight.utils;

import org.junit.Test;
import static org.junit.Assert.*;

public class StreamHealthTest {
    private StreamHealth.Sample sample(long time, boolean fresh, float loss, float decode,
                                      float host, float display, float released, float shown, int heat) {
        return new StreamHealth.Sample(time, fresh, 60, loss, 1, decode, host, display,
                released, released, shown, heat);
    }
    @Test public void identifiesSimultaneousEvidenceWithoutInventingRootCause() {
        StreamHealth health = new StreamHealth();
        for (int n = 1; n <= 5; n++) health.add(sample(n * 1000, true, 4, 20, 25, 50, 60, 40, 2));
        StreamHealth.Report report = health.report(5000);
        assertEquals(5, report.findings.size());
        assertTrue(report.canSuggestBitrate());
        assertEquals(25f, report.worstHostP95Ms, 0f);
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
                Float.POSITIVE_INFINITY, 60, Float.POSITIVE_INFINITY, 0, 0));
        assertTrue(health.report(4000).findings.isEmpty());
        assertEquals(-1, health.report(4000).worstHostP95Ms, 0);
    }
}

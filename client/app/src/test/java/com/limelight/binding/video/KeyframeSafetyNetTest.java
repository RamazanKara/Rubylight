package com.limelight.binding.video;

import org.junit.Test;

import java.util.ArrayList;
import java.util.List;
import java.util.Random;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

/** Erratum 23's repairs against made-up decode latency series, frame by frame at 60 fps. */
public class KeyframeSafetyNetTest {
    private static final int LARGE = 130 * 1024;
    private static final int SMALL = 6 * 1024;

    private interface Series {
        float latencyMs(long ms);
    }

    /** A stream: each frame's size, its decode latency, and a keyframe 50 ms after a request. */
    private static final class Sim {
        final KeyframeSafetyNet.Counts counts = new KeyframeSafetyNet.Counts();
        final KeyframeSafetyNet net = new KeyframeSafetyNet(counts);
        final List<Long> requestsAtMs = new ArrayList<>();
        final Random noise = new Random(23);
        long nowMs;
        long answerAtMs = -1;
        int frameBytes = LARGE;

        Sim() {
            net.onKeyframe(nowMs);
        }

        void run(long forMs, Series series) {
            long start = nowMs;
            while (nowMs - start < forMs) {
                nowMs += 16;
                if (answerAtMs >= 0 && nowMs >= answerAtMs) {
                    answerAtMs = -1;
                    net.onKeyframe(nowMs);
                }
                // A little jitter on every frame, as the reports' lines show.
                net.onDecodeLatency(nowMs, series.latencyMs(nowMs - start) + noise.nextFloat() * 0.5f);
                net.onFrame(frameBytes);
                if (net.due(nowMs)) {
                    requestsAtMs.add(nowMs);
                    answerAtMs = nowMs + 50;
                }
            }
        }

        void steady(long forMs, float ms) {
            run(forMs, at -> ms);
        }

        void calmBaseline() {
            steady(9_000, 3);
        }
    }

    // 3-5-7-8-9 and on to 20, a second a step, as in the report.
    private static float reportersRamp(long ms) {
        float[] steps = {3, 5, 7, 8, 9, 11, 13, 15, 17, 20};
        int step = (int) (ms / 1000);
        return step < steps.length ? steps[step] : 20;
    }

    private static KeyframeSafetyNet inMotion(KeyframeSafetyNet.Counts counts) {
        KeyframeSafetyNet net = new KeyframeSafetyNet(counts);
        for (int i = 0; i < 300; i++) net.onFrame(LARGE);
        return net;
    }

    @Test
    public void theReportersClimbAsksOnceAndOnceWhenItEnds() {
        Sim sim = new Sim();
        sim.calmBaseline();
        long climbStart = sim.nowMs;
        sim.run(22_000, KeyframeSafetyNetTest::reportersRamp);
        assertEquals(1, sim.counts.climbs);
        assertEquals(1, sim.counts.climbRepairs);
        assertEquals(0, sim.counts.climbEndRepairs);
        long firstRepairAfter = sim.requestsAtMs.get(0) - climbStart;
        assertTrue("7 ms comes after 2 s and has to last a second: " + firstRepairAfter,
                firstRepairAfter >= 2_500 && firstRepairAfter <= 4_500);

        sim.run(3_000, at -> 20 - at / 200f);
        sim.steady(8_000, 3);
        assertEquals(1, sim.counts.climbs);
        assertEquals(1, sim.counts.climbRepairs);
        assertEquals(1, sim.counts.climbEndRepairs);
        assertEquals("large frames, inside the limit: nothing else asked", 2, sim.counts.requests());
    }

    @Test
    public void theClimbLogsWhenItStartsAndEnds() {
        Sim sim = new Sim();
        sim.calmBaseline();
        sim.run(16_000, KeyframeSafetyNetTest::reportersRamp);
        String start = sim.net.takeEvent();
        assertTrue(start, start.startsWith("Decoder latency climb: "));
        assertNull("once", sim.net.takeEvent());
        sim.steady(3_000, 3);
        String end = sim.net.takeEvent();
        assertTrue(end, end.startsWith("Decoder latency climb ended after "));
    }

    @Test
    public void singleSpikesAskForNothing() {
        Sim sim = new Sim();
        sim.calmBaseline();
        // One frame in every 60 takes 60 ms, as with the large frames in the reports.
        sim.run(9_000, at -> at / 16 % 60 == 0 ? 60 : 3);
        assertEquals(0, sim.counts.climbs);
        assertEquals(0, sim.counts.requests());
    }

    @Test
    public void aBumpOfHalfASecondAsksForNothing() {
        Sim sim = new Sim();
        sim.calmBaseline();
        sim.run(500, at -> 20);
        sim.steady(9_000, 3);
        assertEquals(0, sim.counts.climbs);
    }

    @Test
    public void aBacklogDrainingIsNoClimb() {
        Sim sim = new Sim();
        sim.calmBaseline();
        sim.run(3_000, at -> 20 - 17 * at / 3000f);
        sim.steady(5_000, 3);
        assertEquals(0, sim.counts.climbs);
    }

    @Test
    public void aPlateauThatSagsALittleIsStillAClimb() {
        Sim sim = new Sim();
        sim.calmBaseline();
        sim.run(2_000, at -> 21 - at / 2000f);
        assertEquals(1, sim.counts.climbs);
    }

    @Test
    public void aFastFrameNowAndThenLeavesTheBaselineAlone() {
        Sim sim = new Sim();
        // The debt lines' 0.3 ms: from a baseline of that, 5 ms would be well above it.
        sim.run(9_000, at -> at / 16 % 30 == 0 ? 0.3f : 3);
        sim.steady(5_000, 5);
        assertEquals(0, sim.counts.climbs);
    }

    @Test
    public void aDriftOverMinutesIsNoClimbAndOnlyTheLimitAsks() {
        Sim sim = new Sim();
        sim.calmBaseline();
        long minutes = 6 * 60_000;
        sim.run(minutes, at -> 3 + 17f * at / minutes);
        assertEquals(0, sim.counts.climbs);
        assertEquals(0, sim.counts.climbRepairs);
        assertEquals(minutes / KeyframeSafetyNet.LIMIT_MS, sim.counts.limitRepairs, 1);
    }

    @Test
    public void aDriftOverAMinuteIsOneClimb() {
        Sim sim = new Sim();
        sim.calmBaseline();
        sim.run(60_000, at -> 3 + 17f * at / 60_000);
        assertEquals(1, sim.counts.climbs);
    }

    @Test
    public void steadyTwentyFromTheStartAsksOnlyAtTheLimit() {
        Sim sim = new Sim();
        sim.steady(61_000, 20);
        assertEquals(0, sim.counts.climbs);
        assertEquals(0, sim.counts.climbRepairs);
        assertEquals(0, sim.counts.quietRepairs);
        assertEquals(2, sim.counts.limitRepairs);
    }

    @Test
    public void aStepToSteadyTwentyAsksOnceAndDoesNotRepeat() {
        Sim sim = new Sim();
        sim.calmBaseline();
        sim.steady(29_000, 20);
        assertEquals(1, sim.counts.climbs);
        assertEquals(1, sim.counts.climbRepairs);
        assertEquals("the limit's wait started over with that repair", 1, sim.counts.requests());
    }

    @Test
    public void aLongClimbIsNoStorm() {
        Sim sim = new Sim();
        sim.calmBaseline();
        sim.steady(120_000, 20);
        assertEquals(1, sim.counts.climbs);
        assertEquals(1, sim.counts.climbRepairs);
        assertEquals(0, sim.counts.climbEndRepairs);
        assertEquals("then only the limit: 40, 70 and 100 s in", 3, sim.counts.limitRepairs);
    }

    @Test
    public void aCalmPictureRefreshesAfterTenSeconds() {
        Sim sim = new Sim();
        sim.frameBytes = SMALL;
        sim.steady(9_500, 3);
        assertEquals(0, sim.counts.requests());
        sim.steady(1_000, 3);
        assertEquals(1, sim.counts.quietRepairs);
        assertEquals(0, sim.counts.limitRepairs);
    }

    @Test
    public void largeFramesWaitForTheLimit() {
        Sim sim = new Sim();
        sim.steady(29_000, 3);
        assertEquals(0, sim.counts.requests());
        sim.steady(2_000, 3);
        assertEquals(1, sim.counts.limitRepairs);
        assertEquals(0, sim.counts.quietRepairs);
    }

    @Test
    public void motionThatCalmsDownRefreshesAboutASecondLater() {
        Sim sim = new Sim();
        sim.steady(15_000, 3);
        assertEquals(0, sim.counts.requests());
        sim.frameBytes = SMALL;
        sim.steady(200, 3);
        assertEquals("the average is still coming down", 0, sim.counts.requests());
        sim.steady(1_800, 3);
        assertEquals(1, sim.counts.quietRepairs);
    }

    @Test
    public void aKeyframeForAnyOtherReasonRestartsTheWaits() {
        KeyframeSafetyNet.Counts counts = new KeyframeSafetyNet.Counts();
        KeyframeSafetyNet net = inMotion(counts);
        net.onKeyframe(0);
        assertFalse(net.due(29_000));
        net.onKeyframe(29_000);
        assertFalse(net.due(58_999));
        assertTrue(net.due(59_000));
        assertEquals(1, counts.limitRepairs);
    }

    @Test
    public void nothingBeforeTheFirstKeyframe() {
        KeyframeSafetyNet net = new KeyframeSafetyNet(new KeyframeSafetyNet.Counts());
        assertFalse(net.due(60_000));
        assertFalse(net.due(120_000));
    }

    @Test
    public void theRateLimitHoldsARepairUntilItsTime() {
        Sim sim = new Sim();
        sim.calmBaseline();
        sim.net.onKeyframe(sim.nowMs);
        long keyframeAt = sim.nowMs;
        sim.steady(2_000, 20);
        assertEquals("the climb is seen", 1, sim.counts.climbs);
        assertEquals("but this soon after a keyframe it waits", 0, sim.counts.climbRepairs);
        sim.steady(4_000, 20);
        assertEquals(1, sim.counts.climbRepairs);
        assertTrue(sim.requestsAtMs.get(0) - keyframeAt >= KeyframeSafetyNet.MIN_GAP_MS);
    }

    @Test
    public void aKeyframeBeforeTheRepairsTurnAnswersIt() {
        Sim sim = new Sim();
        sim.calmBaseline();
        sim.net.onKeyframe(sim.nowMs);
        sim.steady(2_000, 20);
        assertEquals(1, sim.counts.climbs);
        sim.net.onKeyframe(sim.nowMs);
        sim.steady(8_000, 20);
        assertEquals("the keyframe that came is the repair", 0, sim.counts.climbRepairs);
    }

    @Test
    public void aKeyframeDuringAClimbLeavesTheRepairAtItsEnd() {
        Sim sim = new Sim();
        sim.calmBaseline();
        sim.steady(3_000, 20);
        assertEquals(1, sim.counts.climbRepairs);
        sim.net.onKeyframe(sim.nowMs);
        sim.steady(6_000, 20);
        sim.steady(8_000, 3);
        assertEquals(1, sim.counts.climbEndRepairs);
    }

    @Test
    public void anUnansweredRequestWaitsAgainAndDoesNotAskOnEveryFrame() {
        KeyframeSafetyNet.Counts counts = new KeyframeSafetyNet.Counts();
        KeyframeSafetyNet net = inMotion(counts);
        net.onKeyframe(0);
        assertTrue(net.due(30_000));
        for (long at = 30_016; at < 60_000; at += 16) {
            assertFalse(net.due(at));
        }
        assertTrue(net.due(60_000));
        assertEquals(2, counts.limitRepairs);
    }

    @Test
    public void oneGapPerRequest() {
        KeyframeSafetyNet.Counts counts = new KeyframeSafetyNet.Counts();
        KeyframeSafetyNet net = new KeyframeSafetyNet(counts);
        assertFalse("nothing asked yet", net.takeAnsweredGap(4));
        net.onKeyframe(0);
        assertTrue(net.due(30_000));
        assertTrue(net.takeAnsweredGap(4));
        assertFalse(net.takeAnsweredGap(4));
        assertEquals(4, counts.gapFrames);
    }

    @Test
    public void aKeyframeRightBehindTheRequestLeavesNoGapToForgive() {
        KeyframeSafetyNet net = new KeyframeSafetyNet(new KeyframeSafetyNet.Counts());
        net.onKeyframe(0);
        assertTrue(net.due(30_000));
        net.onKeyframe(30_050);
        assertFalse(net.takeAnsweredGap(3));
    }

    @Test
    public void countsOutliveTheNet() {
        KeyframeSafetyNet.Counts counts = new KeyframeSafetyNet.Counts();
        KeyframeSafetyNet first = new KeyframeSafetyNet(counts);
        first.onKeyframe(0);
        assertTrue(first.due(30_000));
        KeyframeSafetyNet restarted = new KeyframeSafetyNet(counts);
        restarted.onKeyframe(40_000);
        assertTrue(restarted.due(70_000));
        assertEquals(2, counts.requests());
        assertEquals("climbs 0, repairs climb 0 end 0 quiet 0 limit 2, gap frames 0, held 0 frames 0 ms", counts.describe());
    }

    @Test
    public void unknownLatencyIsIgnored() {
        Sim sim = new Sim();
        sim.calmBaseline();
        for (int i = 0; i < 600; i++) {
            sim.nowMs += 16;
            sim.net.onDecodeLatency(sim.nowMs, 0);
        }
        // Zeros in the baseline would put 5 ms well above it.
        sim.steady(5_000, 5);
        assertEquals(0, sim.counts.climbs);
    }
}

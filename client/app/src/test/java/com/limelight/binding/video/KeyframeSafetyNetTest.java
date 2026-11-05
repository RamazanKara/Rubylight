package com.limelight.binding.video;

import org.junit.Test;

import java.util.ArrayList;
import java.util.List;
import java.util.Random;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

/** Erratum 23's repairs against made-up decode latency series, frame by frame, at 60 fps unless a test says otherwise. */
public class KeyframeSafetyNetTest {
    private static final int LARGE = 130 * 1024;
    private static final int SMALL = 6 * 1024;
    private static final int FPS_30 = 30_000;
    private static final int FPS_60 = 60_000;
    private static final int FPS_120 = 120_000;
    private static final int KBPS = 63_000;

    private interface Series {
        float latencyMs(long ms);
    }

    /** A stream: each frame's size, its decode latency, and a keyframe 50 ms after a request. */
    private static final class Sim {
        final KeyframeSafetyNet.Counts counts = new KeyframeSafetyNet.Counts();
        final KeyframeSafetyNet net = new KeyframeSafetyNet(counts);
        final List<Long> requestsAtMs = new ArrayList<>();
        final List<Long> climbRepairsAtMs = new ArrayList<>();
        final Random noise = new Random(23);
        final int fpsMilliHz;
        final long frameMs;
        long nowMs;
        long answerAtMs = -1;
        int frameBytes = LARGE;
        int bitrateKbps = KBPS;

        Sim() {
            this(FPS_60, 16);
        }

        Sim(int fpsMilliHz, long frameMs) {
            this.fpsMilliHz = fpsMilliHz;
            this.frameMs = frameMs;
            net.onKeyframe(nowMs);
        }

        private void frame(float latencyMs) {
            nowMs += frameMs;
            if (answerAtMs >= 0 && nowMs >= answerAtMs) {
                answerAtMs = -1;
                net.onKeyframe(nowMs);
            }
            net.onDecodeLatency(nowMs, latencyMs, fpsMilliHz);
            net.onFrame(frameBytes);
            int climbRepairsBefore = counts.climbRepairs + counts.climbEndRepairs;
            if (net.due(nowMs, bitrateKbps, fpsMilliHz)) {
                requestsAtMs.add(nowMs);
                if (counts.climbRepairs + counts.climbEndRepairs > climbRepairsBefore) climbRepairsAtMs.add(nowMs);
                answerAtMs = nowMs + 50;
            }
        }

        void run(long forMs, Series series) {
            long start = nowMs;
            while (nowMs - start < forMs) {
                // A little jitter on every frame, as the reports' lines show.
                frame(series.latencyMs(nowMs + frameMs - start) + noise.nextFloat() * 0.5f);
            }
        }

        void steady(long forMs, float ms) {
            run(forMs, at -> ms);
        }

        /** Whole buckets of exactly this latency, at 16 ms a frame 17 frames close one; used alone they stay aligned. */
        void buckets(int count, float ms) {
            for (int i = 0; i < count * 17; i++) frame(ms);
        }

        void pause(long forMs) {
            nowMs += forMs;
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

    private static boolean due(KeyframeSafetyNet net, long nowMs) {
        return net.due(nowMs, KBPS, FPS_60);
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
        assertTrue("9 ms is the first step past the floor, after 4 s, and has to last a second: " + firstRepairAfter,
                firstRepairAfter >= 4_500 && firstRepairAfter <= 6_000);

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
        // At 120 fps, where the floor leaves the baseline to decide. The debt lines' 0.3 ms: from a
        // baseline of that, 5 ms would be well above it.
        Sim sim = new Sim(FPS_120, 8);
        sim.run(9_000, at -> at / 8 % 30 == 0 ? 0.3f : 3);
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
    public void aLatencyMovingInsideTheFloorIsNoClimb() {
        Sim sim = new Sim();
        sim.calmBaseline();
        // 3 to 7.5 ms and back every 3 s. Without the floor that is a climb and a keyframe every 5 s for good.
        sim.run(120_000, at -> at % 3000 < 1500 ? 7.5f : 3);
        assertEquals(0, sim.counts.climbs);
        assertEquals("only the limit", sim.counts.limitRepairs, sim.counts.requests());
    }

    private static int climbsAt(int fpsMilliHz, long frameMs, float level) {
        Sim sim = new Sim(fpsMilliHz, frameMs);
        sim.steady(9_000, 3);
        sim.steady(3_000, level);
        return sim.counts.climbs;
    }

    @Test
    public void theFloorIsHalfTheFrameIntervalAtEveryFrameRate() {
        assertEquals("under 16.7 ms", 0, climbsAt(FPS_30, 33, 15));
        assertEquals(1, climbsAt(FPS_30, 33, 18));
        assertEquals("under 8.3 ms", 0, climbsAt(FPS_60, 16, 7.5f));
        assertEquals(1, climbsAt(FPS_60, 16, 9));
        // At 120 fps the floor is 4.2 ms, so being well above the 3 ms baseline decides: 7 ms.
        assertEquals(0, climbsAt(FPS_120, 8, 6));
        assertEquals("over the floor that 60 fps has", 1, climbsAt(FPS_120, 8, 8));
    }

    @Test
    public void anUnknownFrameRateHasNoClimbsAndStillTheLimit() {
        Sim sim = new Sim(0, 16);
        sim.calmBaseline();
        sim.steady(25_000, 30);
        assertEquals(0, sim.counts.climbs);
        assertEquals(1, sim.counts.limitRepairs);
    }

    private static void assertReportersClimbAsks(int fpsMilliHz, long frameMs) {
        Sim sim = new Sim(fpsMilliHz, frameMs);
        sim.calmBaseline();
        sim.run(22_000, KeyframeSafetyNetTest::reportersRamp);
        assertEquals(1, sim.counts.climbs);
        assertEquals(1, sim.counts.climbRepairs);
        sim.steady(8_000, 3);
        assertEquals(1, sim.counts.climbEndRepairs);
    }

    @Test
    public void theReportersClimbAsksAt30And120Fps() {
        assertReportersClimbAsks(FPS_30, 33);
        assertReportersClimbAsks(FPS_120, 8);
    }

    @Test
    public void aLatencyThatKeepsMovingBacksOffToTheLimitsPace() {
        Sim sim = new Sim();
        sim.calmBaseline();
        // Past the floor this time: a climb and its end every 3 s, for five minutes.
        sim.run(300_000, at -> at % 3000 < 1500 ? 12 : 3);
        assertTrue("climbs " + sim.counts.climbs, sim.counts.climbs > 50);
        long[] gaps = {5_000, 10_000, 20_000, 30_000, 30_000, 30_000};
        for (int i = 0; i < gaps.length; i++) {
            long gap = sim.climbRepairsAtMs.get(i + 1) - sim.climbRepairsAtMs.get(i);
            assertTrue("repair " + i + " to the next: " + gap, gap >= gaps[i] && gap < gaps[i] + 3_100);
        }
        assertEquals("nothing else got in", sim.climbRepairsAtMs.size(), sim.counts.requests());
        assertTrue("requests " + sim.counts.requests(), sim.counts.requests() <= 3 + 300 / 30);
    }

    // Climbs of 12 s every 45 s: each start's repair would use up the gap, and the end's would land up to 22 s
    // late. Once backed off only the end asks, right after the climb.
    @Test
    public void repeatedClimbsEndOnARepairOnTime() {
        Sim sim = new Sim();
        sim.calmBaseline();
        long start = sim.nowMs;
        int cycles = 8;
        sim.run(cycles * 45_000L, at -> at % 45_000 < 12_000 ? 20 : 3);
        assertEquals(cycles, sim.counts.climbs);
        assertEquals("only the first start asks", 1, sim.counts.climbRepairs);
        assertEquals("every end asks", cycles, sim.counts.climbEndRepairs);
        for (int i = 0; i < cycles; i++) {
            long endMs = start + i * 45_000L + 12_000;
            long repairMs = sim.climbRepairsAtMs.get(i + 1);
            assertTrue("cycle " + i + ": " + (repairMs - endMs) + " ms after the climb",
                    repairMs >= endMs && repairMs < endMs + 2_000);
        }
    }

    private static Sim movedUntilTheGapsAreLong() {
        Sim sim = new Sim();
        sim.calmBaseline();
        sim.run(120_000, at -> at % 3000 < 1500 ? 12 : 3);
        return sim;
    }

    @Test
    public void aCalmStretchStartsTheGapsOver() {
        Sim sim = movedUntilTheGapsAreLong();
        sim.steady(95_000, 3);
        int before = sim.climbRepairsAtMs.size();
        sim.steady(7_000, 20);
        assertEquals("the climb asks", before + 1, sim.climbRepairsAtMs.size());
        sim.steady(8_000, 3);
        assertEquals("and its end", before + 2, sim.climbRepairsAtMs.size());
        long gap = sim.climbRepairsAtMs.get(before + 1) - sim.climbRepairsAtMs.get(before);
        assertTrue("the first gap again: " + gap, gap < 10_000);
    }

    @Test
    public void aShortCalmKeepsTheGapsLong() {
        Sim sim = movedUntilTheGapsAreLong();
        sim.steady(40_000, 3);
        int before = sim.climbRepairsAtMs.size();
        sim.steady(7_000, 20);
        sim.steady(8_000, 3);
        sim.steady(40_000, 3);
        assertTrue(sim.climbRepairsAtMs.size() > before);
        long gap = sim.climbRepairsAtMs.get(before) - sim.climbRepairsAtMs.get(before - 1);
        assertTrue("still the long gap: " + gap, gap >= 30_000);
    }

    @Test
    public void aRepairWaitingForItsGapLeavesTheOtherWaitsRunning() {
        Sim sim = movedUntilTheGapsAreLong();
        // Only 3 ms left, so nothing more climbs, and the calm picture's refresh comes before the long gap ends.
        sim.frameBytes = SMALL;
        sim.steady(2_000, 3);
        int quiet = sim.counts.quietRepairs;
        sim.steady(30_000, 3);
        assertTrue("quiet " + sim.counts.quietRepairs, sim.counts.quietRepairs > quiet);
    }

    @Test
    public void aLevelThatStaysBecomesTheBaselineWithoutARepair() {
        Sim sim = new Sim();
        // A calm desktop to warm up on, then a game.
        sim.steady(9_000, 1.5f);
        sim.steady(70_000, 12);
        assertEquals(1, sim.counts.climbs);
        assertEquals(1, sim.counts.settledClimbs);
        assertEquals(1, sim.counts.climbRepairs);
        assertEquals("no repair for that", 0, sim.counts.climbEndRepairs);
        String event = sim.net.takeEvent();
        assertTrue(event, event.startsWith("Decoder latency stayed up for "));

        sim.steady(100_000, 12);
        assertEquals("that level is calm now", 1, sim.counts.climbs);
        sim.steady(3_000, 30);
        assertEquals("and the stream can climb from it", 2, sim.counts.climbs);
        assertEquals(2, sim.counts.climbRepairs);
    }

    @Test
    public void aGameAtSixToEightMsSettlesAt120Fps() {
        Sim sim = new Sim(FPS_120, 8);
        sim.steady(9_000, 1.5f);
        sim.run(70_000, at -> at % 2000 < 1000 ? 6 : 8);
        assertEquals(1, sim.counts.climbs);
        assertEquals(1, sim.counts.settledClimbs);
        sim.steady(3_000, 20);
        assertEquals(2, sim.counts.climbs);
    }

    // Twice a settled 12 ms is 24 ms, so 20 ms, over the 16.7 ms a frame has, would be no climb.
    @Test
    public void anOverrunAfterSettlingIsAClimb() {
        Sim sim = new Sim();
        sim.steady(9_000, 1.5f);
        sim.steady(70_000, 12);
        assertEquals(1, sim.counts.settledClimbs);
        sim.steady(3_000, 12);
        assertEquals(1, sim.counts.climbs);
        sim.steady(3_000, 20);
        assertEquals(2, sim.counts.climbs);
    }

    @Test
    public void anOverrunFromAHighBaselineIsAClimbAt120Fps() {
        Sim sim = new Sim(FPS_120, 8);
        sim.steady(9_000, 5);
        // Under the 10 ms of twice the baseline, over the 8.3 ms a frame has and 4 ms over the baseline.
        sim.steady(3_000, 9.5f);
        assertEquals(1, sim.counts.climbs);
    }

    @Test
    public void aSettledLevelStillMovesInsideTheFrameWithoutAClimb() {
        Sim sim = new Sim();
        sim.steady(9_000, 1.5f);
        sim.steady(70_000, 12);
        sim.run(120_000, at -> at % 3000 < 1500 ? 15 : 12);
        assertEquals(1, sim.counts.climbs);
    }

    @Test
    public void aSettledLevelMovingPastTheFrameIsNoStorm() {
        Sim sim = new Sim();
        sim.steady(9_000, 1.5f);
        sim.steady(70_000, 12);
        int before = sim.counts.requests();
        sim.run(300_000, at -> at % 3000 < 1500 ? 18 : 12);
        assertTrue("climbs " + sim.counts.climbs, sim.counts.climbs > 50);
        assertTrue("requests " + (sim.counts.requests() - before), sim.counts.requests() - before <= 3 + 300 / 30);
    }

    @Test
    public void aLevelThatFallsAfterSettlingAsksForNothing() {
        Sim sim = new Sim();
        sim.calmBaseline();
        sim.steady(70_000, 20);
        assertEquals(1, sim.counts.settledClimbs);
        sim.steady(8_000, 3);
        assertEquals(0, sim.counts.climbEndRepairs);
        assertEquals(1, sim.counts.climbs);
    }

    @Test
    public void aClimbThatEndsInsideTheMinuteKeepsItsRepair() {
        Sim sim = new Sim();
        sim.calmBaseline();
        sim.steady(50_000, 20);
        sim.steady(8_000, 3);
        assertEquals(0, sim.counts.settledClimbs);
        assertEquals(1, sim.counts.climbEndRepairs);
    }

    @Test
    public void oneBucketsSpikeLeavesTheWayBackDownAlone() {
        Sim sim = new Sim();
        sim.buckets(36, 3);
        sim.buckets(8, 20);
        assertEquals(1, sim.counts.climbs);
        sim.buckets(1, 60);
        // With 60 ms as the peak the way back down would be 31.5 ms, so 20 ms would read as the climb's end.
        sim.buckets(12, 20);
        assertEquals(1, sim.counts.climbs);
        assertEquals(0, sim.counts.climbEndRepairs);
        sim.buckets(8, 3);
        assertEquals(1, sim.counts.climbEndRepairs);
        assertEquals(1, sim.counts.climbs);
    }

    @Test
    public void aSpikeOnTheBucketThatStartsAClimbIsNoPeak() {
        Sim sim = new Sim();
        sim.buckets(36, 3);
        // Four buckets are under the second's hold, the fifth ends it.
        sim.buckets(4, 10);
        sim.buckets(1, 60);
        assertEquals(1, sim.counts.climbs);
        // With 60 ms as the peak the way back down would be 31.5 ms, so 10 ms would read as the climb's end.
        sim.buckets(12, 10);
        assertEquals(0, sim.counts.climbEndRepairs);
        sim.buckets(8, 3);
        assertEquals(1, sim.counts.climbEndRepairs);
    }

    @Test
    public void aPeakThatHoldsMovesTheWayBackDown() {
        Sim sim = new Sim();
        sim.buckets(36, 3);
        sim.buckets(8, 20);
        sim.buckets(8, 60);
        sim.buckets(8, 35);
        assertEquals("over the way back down from 60 ms", 0, sim.counts.climbEndRepairs);
        sim.buckets(8, 20);
        assertEquals("under it", 1, sim.counts.climbEndRepairs);
    }

    @Test
    public void aPauseWithAnEmptyBucketStartsTheHoldOver() {
        Sim sim = new Sim();
        sim.buckets(36, 3);
        sim.buckets(2, 20);
        sim.pause(3_000);
        sim.buckets(1, 20);
        assertEquals("the second before the pause doesn't count", 0, sim.counts.climbs);
        sim.buckets(5, 20);
        assertEquals("a whole new hold does", 1, sim.counts.climbs);
    }

    @Test
    public void aPauseWithFramesInTheBucketStartsTheHoldOver() {
        Sim sim = new Sim();
        sim.buckets(36, 3);
        sim.buckets(2, 20);
        for (int i = 0; i < 3; i++) sim.frame(20);
        sim.pause(3_000);
        sim.buckets(1, 20);
        assertEquals(0, sim.counts.climbs);
        sim.buckets(5, 20);
        assertEquals(1, sim.counts.climbs);
    }

    @Test
    public void aPauseStartsTheEndsHoldOverToo() {
        Sim sim = new Sim();
        sim.buckets(36, 3);
        sim.buckets(8, 20);
        sim.buckets(2, 3);
        sim.pause(3_000);
        sim.buckets(1, 3);
        assertEquals(0, sim.counts.climbEndRepairs);
        sim.buckets(5, 3);
        assertEquals(1, sim.counts.climbEndRepairs);
    }

    @Test
    public void aPauseIsNoTimeAtALevel() {
        Sim sim = new Sim();
        sim.calmBaseline();
        sim.steady(40_000, 20);
        sim.pause(40_000);
        sim.steady(15_000, 20);
        assertEquals(0, sim.counts.settledClimbs);
        sim.steady(10_000, 20);
        assertEquals(1, sim.counts.settledClimbs);
    }

    @Test
    public void calmIsAQuarterOfAFramesShareOfTheBitrate() {
        assertEquals(32_812, KeyframeSafetyNet.quietFrameBytes(63_000, FPS_60));
        assertEquals(16_406, KeyframeSafetyNet.quietFrameBytes(63_000, FPS_120));
        assertEquals(62_500, KeyframeSafetyNet.quietFrameBytes(60_000, FPS_30));
        assertEquals(5_208, KeyframeSafetyNet.quietFrameBytes(10_000, FPS_60));
        assertEquals("the cap", 64 * 1024, KeyframeSafetyNet.quietFrameBytes(160_000, FPS_60));
        assertEquals("the cap", 64 * 1024, KeyframeSafetyNet.quietFrameBytes(300_000, FPS_60));
        assertEquals("the cap", 64 * 1024, KeyframeSafetyNet.quietFrameBytes(63_000, 24_000));
        assertEquals("no bitrate", 0, KeyframeSafetyNet.quietFrameBytes(0, FPS_60));
        assertEquals("no frame rate", 0, KeyframeSafetyNet.quietFrameBytes(63_000, 0));
    }

    private static void assertFullMotionIsNotCalm(int fpsMilliHz, long frameMs, int bitrateKbps, int frameBytes) {
        Sim sim = new Sim(fpsMilliHz, frameMs);
        sim.bitrateKbps = bitrateKbps;
        sim.frameBytes = frameBytes;
        sim.steady(29_000, 3);
        assertEquals("every " + frameBytes + " B frame is what the bitrate allows", 0, sim.counts.requests());
        sim.steady(2_000, 3);
        assertEquals(1, sim.counts.limitRepairs);
    }

    @Test
    public void fullMotionAtALowBitrateIsNotCalm() {
        assertFullMotionIsNotCalm(FPS_60, 16, 10_000, 20_833);
        assertFullMotionIsNotCalm(FPS_60, 16, 15_000, 31_250);
        assertFullMotionIsNotCalm(FPS_120, 8, 30_000, 31_250);
    }

    @Test
    public void aCalmPictureAtALowBitrateStillRefreshes() {
        Sim sim = new Sim();
        sim.bitrateKbps = 10_000;
        sim.frameBytes = 3 * 1024;
        sim.steady(9_500, 3);
        assertEquals(0, sim.counts.requests());
        sim.steady(1_000, 3);
        assertEquals(1, sim.counts.quietRepairs);
    }

    @Test
    public void aHighBitrateCallsLargerFramesCalm() {
        // Over 63 Mbps' 32 KB and under the cap. It used to prove 150 Mbps' whole share, about 78 KB; the
        // 64 KiB cap now ends that, since a share of a high bitrate called moderate motion calm.
        Sim sim = new Sim();
        sim.bitrateKbps = 150_000;
        sim.frameBytes = 60 * 1024;
        sim.steady(10_500, 3);
        assertEquals(1, sim.counts.quietRepairs);
    }

    @Test
    public void realMotionAtAHighBitrateIsNotCalm() {
        assertFullMotionIsNotCalm(FPS_60, 16, 160_000, 70 * 1024);
        assertFullMotionIsNotCalm(FPS_60, 16, 300_000, 130 * 1024);
    }

    @Test
    public void realMotionAt24FpsIsNotCalmAndACalmPictureIs() {
        assertFullMotionIsNotCalm(24_000, 42, 63_000, 70 * 1024);
        Sim sim = new Sim(24_000, 42);
        sim.frameBytes = 30 * 1024;
        sim.steady(10_500, 3);
        assertEquals(1, sim.counts.quietRepairs);
    }

    @Test
    public void aClimbAt24FpsGoesByItsLongerFrame() {
        assertEquals("under 20.8 ms", 0, climbsAt(24_000, 42, 18));
        assertEquals(1, climbsAt(24_000, 42, 25));
    }

    @Test
    public void aBitrateCutChangesWhatCalmIsAtOnce() {
        Sim sim = new Sim();
        sim.frameBytes = 25 * 1024;
        sim.bitrateKbps = 30_000;
        sim.steady(15_000, 3);
        assertEquals("over a quarter of 30 Mbps' frame", 0, sim.counts.requests());
        sim.bitrateKbps = KBPS;
        sim.steady(100, 3);
        assertEquals("under a quarter of 63 Mbps' frame", 1, sim.counts.quietRepairs);
    }

    @Test
    public void anUnknownBitrateIsNeverCalm() {
        Sim sim = new Sim();
        sim.bitrateKbps = 0;
        sim.frameBytes = SMALL;
        sim.steady(29_000, 3);
        assertEquals(0, sim.counts.requests());
        sim.steady(2_000, 3);
        assertEquals(1, sim.counts.limitRepairs);
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
        assertFalse(due(net, 29_000));
        net.onKeyframe(29_000);
        assertFalse(due(net, 58_999));
        assertTrue(due(net, 59_000));
        assertEquals(1, counts.limitRepairs);
    }

    @Test
    public void nothingBeforeTheFirstKeyframe() {
        KeyframeSafetyNet net = new KeyframeSafetyNet(new KeyframeSafetyNet.Counts());
        assertFalse(due(net, 60_000));
        assertFalse(due(net, 120_000));
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
        assertTrue(due(net, 30_000));
        for (long at = 30_016; at < 60_000; at += 16) {
            assertFalse(due(net, at));
        }
        assertTrue(due(net, 60_000));
        assertEquals(2, counts.limitRepairs);
    }

    @Test
    public void oneGapPerRequest() {
        KeyframeSafetyNet.Counts counts = new KeyframeSafetyNet.Counts();
        KeyframeSafetyNet net = new KeyframeSafetyNet(counts);
        assertFalse("nothing asked yet", net.takeAnsweredGap(4));
        net.onKeyframe(0);
        assertTrue(due(net, 30_000));
        assertTrue(net.takeAnsweredGap(4));
        assertFalse(net.takeAnsweredGap(4));
        assertEquals(4, counts.gapFrames);
    }

    @Test
    public void aKeyframeRightBehindTheRequestLeavesNoGapToForgive() {
        KeyframeSafetyNet net = new KeyframeSafetyNet(new KeyframeSafetyNet.Counts());
        net.onKeyframe(0);
        assertTrue(due(net, 30_000));
        net.onKeyframe(30_050);
        assertFalse(net.takeAnsweredGap(3));
    }

    @Test
    public void countsOutliveTheNet() {
        KeyframeSafetyNet.Counts counts = new KeyframeSafetyNet.Counts();
        KeyframeSafetyNet first = new KeyframeSafetyNet(counts);
        first.onKeyframe(0);
        assertTrue(due(first, 30_000));
        KeyframeSafetyNet restarted = new KeyframeSafetyNet(counts);
        restarted.onKeyframe(40_000);
        assertTrue(due(restarted, 70_000));
        assertEquals(2, counts.requests());
        assertEquals("climbs 0, settled 0, repairs climb 0 end 0 quiet 0 limit 2, gap frames 0, held 0 frames 0 ms", counts.describe());
    }

    @Test
    public void unknownLatencyIsIgnored() {
        // At 120 fps, where the floor leaves the baseline to decide.
        Sim sim = new Sim(FPS_120, 8);
        sim.calmBaseline();
        for (int i = 0; i < 1200; i++) {
            sim.nowMs += 8;
            sim.net.onDecodeLatency(sim.nowMs, 0, FPS_120);
        }
        // Zeros in the baseline would put 5 ms well above it.
        sim.steady(5_000, 5);
        assertEquals(0, sim.counts.climbs);
    }
}

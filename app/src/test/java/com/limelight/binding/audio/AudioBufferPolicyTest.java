package com.limelight.binding.audio;

import org.junit.Test;
import static org.junit.Assert.*;

public class AudioBufferPolicyTest {
    /** What a simulated stream did: drops by kind, and the audio waiting late in the run. */
    private static final class DriftRun {
        int trims, lateDrops, underruns;
        // The most and least audio waiting (network queue, writer, ring) over the last quarter of the run,
        // in frames: the least is the latency that stays.
        int lateMaxFrames, lateMinFrames = Integer.MAX_VALUE;
    }

    /**
     * An AAudio stream as aaudio_output.c plays it: 5 ms packets in pairs every 10 ms from the host, a
     * blocking writer into a ring capped at the reserve, a device taking 4 ms bursts on its own clock,
     * slower by ppm (negative: faster). heldMs(pair) holds a pair back (a jitter burst); the network
     * hands packets over in order, so those behind it wait too.
     */
    private static DriftRun simulate(int seconds, double ppm, java.util.function.IntUnaryOperator heldMs, boolean trim) {
        final int rate = 48000, packet = 240, burst = 192;
        int reserve = AudioBufferPolicy.reserveStartFrames(burst, rate);
        AudioBufferPolicy.DriftTrim drift = new AudioBufferPolicy.DriftTrim();
        DriftRun run = new DriftRun();
        java.util.ArrayDeque<Long> network = new java.util.ArrayDeque<>();
        long endUs = seconds * 1000000L;
        double burstUs = 4000 * (1 + ppm / 1e6), nextBurstUs = 0;
        long nextPairUs = 0, releasedUs = 0;
        int pair = 0, ring = 0, writing = 0;
        boolean primed = false;
        for (long now = 0; now < endUs; now += 100) {
            while (nextPairUs <= now) {
                releasedUs = Math.max(releasedUs, nextPairUs + heldMs.applyAsInt(pair++) * 1000L);
                network.add(releasedUs);
                network.add(releasedUs);
                nextPairUs += 10000;
            }
            while (writing == 0 && !network.isEmpty() && network.peek() <= now) {
                network.poll();
                int pendingMs = 0;
                for (long at : network) if (at <= now) pendingMs += 5;
                if (AudioBufferPolicy.dropLate(pendingMs, ring, reserve, rate)) run.lateDrops++;
                else if (trim && drift.trim(pendingMs * rate / 1000, ring, reserve, packet, rate)) run.trims++;
                else writing = packet;
            }
            int room = Math.min(writing, reserve - ring);
            ring += room;
            writing -= room;
            if (!primed && ring >= reserve) primed = true;
            if (now >= nextBurstUs) {
                nextBurstUs += burstUs;
                if (primed) {
                    if (ring < burst) {
                        run.underruns++;
                        primed = false;
                    }
                    ring -= Math.min(ring, burst);
                }
            }
            if (now >= endUs * 3 / 4) {
                int waiting = ring + writing;
                for (long at : network) if (at <= now) waiting += packet;
                run.lateMaxFrames = Math.max(run.lateMaxFrames, waiting);
                run.lateMinFrames = Math.min(run.lateMinFrames, waiting);
            }
        }
        return run;
    }

    @Test public void aSlowDeviceClockIsTrimmedAPacketAtATimeInsteadOfCreepingToTheLateBound() {
        // 35 ppm measured: 42 ms of surplus in 20 minutes. Without the trim it reaches the 40 ms late bound within 30.
        DriftRun untrimmed = simulate(1800, 35, pair -> 0, false);
        assertTrue("untrimmed: " + untrimmed.lateDrops + " late drops", untrimmed.lateDrops > 0);
        DriftRun steady = simulate(60, 0, pair -> 0, true);
        for (double ppm : new double[] {35, 300}) {
            DriftRun run = simulate(ppm > 100 ? 120 : 1200, ppm, pair -> 0, true);
            assertEquals(ppm + " ppm", 0, run.lateDrops);
            // 42 ms or 36 ms of surplus, 5 ms a trim, less what stays below the trim line.
            assertTrue(ppm + " ppm: " + run.trims + " trims", run.trims >= 4 && run.trims <= 10);
            // What stays is at most a packet or two over steady state, never the 40 ms the late bound allows.
            assertTrue(ppm + " ppm: at least " + run.lateMinFrames + " frames waiting, " + steady.lateMinFrames +
                    " steady", run.lateMinFrames <= steady.lateMinFrames + 2 * 240);
            assertTrue(ppm + " ppm: at most " + run.lateMaxFrames + " frames waiting, " + steady.lateMaxFrames +
                    " steady", run.lateMaxFrames <= steady.lateMaxFrames + 4 * 240);
        }
    }

    @Test public void steadyStateFastDevicesAndJitterBurstsAreNeverTrimmed() {
        // A 30 ms hold every 3 s, and every 0.5 s a 12 ms one: the burst after it refills, nothing is surplus.
        java.util.function.IntUnaryOperator jitter = pair -> pair % 300 == 7 ? 30 : pair % 50 == 3 ? 12 : 0;
        for (double ppm : new double[] {0, -35, -300}) {
            for (boolean bursts : new boolean[] {false, true}) {
                DriftRun run = simulate(300, ppm, bursts ? jitter : pair -> 0, true);
                assertEquals(ppm + " ppm, bursts " + bursts, 0, run.trims);
                assertEquals(ppm + " ppm, bursts " + bursts, 0, run.lateDrops);
            }
        }
        // Jitter on top of drift still trims only the drift.
        DriftRun run = simulate(1200, 35, jitter, true);
        assertEquals(0, run.lateDrops);
        assertTrue(run.trims + " trims", run.trims >= 6 && run.trims <= 10);
    }

    @Test public void theTrimNeedsTwoSecondsAboveTheLineInARow() {
        AudioBufferPolicy.DriftTrim trim = new AudioBufferPolicy.DriftTrim();
        // 400 packets of 5 ms are 2 s: the 400th trims, a dip on the way starts over.
        for (int i = 1; i < 400; i++) assertFalse(trim.trim(481, 672, 672, 240, 48000));
        assertFalse("one packet above the reserve is not surplus", trim.trim(240, 672, 672, 240, 48000));
        for (int i = 1; i < 400; i++) assertFalse(trim.trim(481, 672, 672, 240, 48000));
        assertTrue(trim.trim(481, 672, 672, 240, 48000));
        assertFalse("starts over after a trim", trim.trim(481, 672, 672, 240, 48000));
        trim.reset();
        assertFalse(trim.trim(481, 672, 672, 0, 48000));
        assertFalse(trim.trim(481, 672, 672, 240, 0));
    }

    @Test public void aaudioIsReopenedUntilItKeepsDyingRightAfterReopening() {
        assertTrue(AudioBufferPolicy.reopenAAudio(0));
        assertTrue(AudioBufferPolicy.reopenAAudio(AudioBufferPolicy.MAX_QUICK_REOPENS - 1));
        assertFalse(AudioBufferPolicy.reopenAAudio(AudioBufferPolicy.MAX_QUICK_REOPENS));
    }
    @Test public void underrunsGrowOneBurstAndCleanPlaybackRecoversSlowly() {
        AudioBufferPolicy policy = new AudioBufferPolicy(192, 1920, 384, 0, 0);
        assertEquals(384, policy.sample(10, 999));
        assertEquals(576, policy.sample(10, 1000));
        assertEquals(768, policy.sample(11, 2000));
        assertEquals(768, policy.sample(11, 31000));
        assertEquals(576, policy.sample(11, 32000));
        assertEquals(384, policy.sample(11, 62000));
        assertEquals(384, policy.sample(11, 92000));
    }

    @Test public void growthIsBoundedByCapacityAndEightBursts() {
        for (int capacity : new int[] {576, 1920}) {
            AudioBufferPolicy policy = new AudioBufferPolicy(192, capacity, 384, 0, 0);
            int frames = 0;
            for (int i = 1; i < 100; i++) frames = policy.sample(i, i * 1000L);
            assertEquals(Math.min(capacity, 1536), frames);
        }
    }

    @Test public void counterResetsAndMissingMeasurementsCannotTriggerShrink() {
        AudioBufferPolicy policy = new AudioBufferPolicy(240, 2400, 480, 10, 0);
        assertEquals(720, policy.sample(11, 1000));
        assertEquals(720, policy.sample(-1, 31000));
        assertEquals(720, policy.sample(0, 32000));
        policy.applied(960);
        assertEquals(960, policy.sample(0, 33000));
    }

    @Test public void platformMinimumAndRejectedSizeRequestsRemainSafe() {
        AudioBufferPolicy policy = new AudioBufferPolicy(120, 2400, 1440, 0, 0);
        assertEquals(1440, policy.sample(0, 30000));
        policy.applied(-1);
        assertEquals(1440, policy.sample(1, 31000));
    }

    @Test public void oldApiSurroundEffectsAndResamplingUseAudioTrack() {
        assertTrue(AudioBufferPolicy.useAAudio(27, 2, false, 48000, 48000));
        assertFalse(AudioBufferPolicy.useAAudio(26, 2, false, 48000, 48000));
        assertFalse(AudioBufferPolicy.useAAudio(36, 6, false, 48000, 48000));
        assertFalse(AudioBufferPolicy.useAAudio(36, 8, false, 48000, 48000));
        assertFalse(AudioBufferPolicy.useAAudio(36, 2, true, 48000, 48000));
        assertFalse(AudioBufferPolicy.useAAudio(36, 2, false, 48000, 44100));
    }

    @Test public void repeatedStarvationAtTheLimitRequestsFallback() {
        AudioBufferPolicy policy = new AudioBufferPolicy(192, 576, 384, 0, 0);
        assertEquals(576, policy.sample(1, 1000));
        assertFalse(policy.needsFallback());
        for (int i = 2; i <= 4; i++) {
            policy.sample(i * 10, i * 5000L);
            assertEquals(i == 4, policy.needsFallback());
        }
    }

    // Odin 2 Portal in #15: packet 240, fast path denied. Its capacity 2886 = 3 * (960 + 2) is AOSP's
    // calculateMinFrameCount, so getMinBufferSize is taken as 2886 frames, with a granted capacity to match.
    private static final int ODIN_MIN_BUFFER = 2886;
    private static final int ODIN_CAPACITY = AudioBufferPolicy.trackCapacityFrames(240, ODIN_MIN_BUFFER, 192);

    private static AudioBufferPolicy deniedFastPath() {
        int floor = AudioBufferPolicy.trackFloorFrames(33, AudioBufferPolicy.PERF_NONE, true, 240, ODIN_MIN_BUFFER, 192);
        int step = AudioBufferPolicy.trackStepFrames(false, 240, 192);
        return AudioBufferPolicy.forTrack(step, ODIN_CAPACITY, floor, 480, 0, 0, AudioBufferPolicy.maxReserveFrames(48000));
    }

    @Test public void deniedFastPathFloorsAtTheFrameworkMinimum() {
        assertEquals(2886, AudioBufferPolicy.trackFloorFrames(33, AudioBufferPolicy.PERF_NONE, true, 240, 2886, 192));
        assertEquals(2886, AudioBufferPolicy.trackFloorFrames(33, AudioBufferPolicy.PERF_POWER_SAVING, false, 240, 2886, 192));
        // A tiny platform minimum never drops below two packets.
        assertEquals(480, AudioBufferPolicy.trackFloorFrames(33, AudioBufferPolicy.PERF_NONE, false, 240, 100, 192));
        // Android 7.x cannot report the mode: a plain track is a normal-mixer track.
        assertEquals(2886, AudioBufferPolicy.trackFloorFrames(25, -1, false, 240, 2886, 192));
    }

    @Test public void deniedFastPathNeverShrinksBelowTheFloor() {
        AudioBufferPolicy policy = deniedFastPath();
        // The 480 frames requested in #15 are far below the floor.
        assertEquals(2886, policy.sample(0, 1000));
        for (long t = 30000; t < 600000; t += 1000) assertEquals(2886, policy.sample(0, t));
    }

    @Test public void deniedFastPathStillGrowsOnUnderrunsAndShrinksBackToTheFloor() {
        AudioBufferPolicy policy = deniedFastPath();
        assertEquals(2886, policy.sample(0, 1000));
        assertEquals(3126, policy.sample(5, 2000));
        assertEquals(3366, policy.sample(9, 3000));
        // Growth up to six steps above the floor, as before: 40 ms is less than that.
        int frames = 0;
        for (int i = 1; i < 50; i++) frames = policy.sample(10 + i, 3000 + i * 1000L);
        assertEquals(2886 + 6 * 240, frames);
        assertTrue(frames <= ODIN_CAPACITY);
        policy.applied(3366);
        assertEquals(3366, policy.sample(59, 60000));
        assertEquals(3126, policy.sample(59, 83000));
        assertEquals(2886, policy.sample(59, 114000));
        assertEquals(2886, policy.sample(59, 145000));
    }

    @Test public void fastTrackKeepsTheSmallFloor() {
        // The two-packet floor stays whatever the output burst or the framework minimum.
        assertEquals(480, AudioBufferPolicy.trackFloorFrames(33, AudioBufferPolicy.PERF_LOW_LATENCY, true, 240, 2886, 192));
        assertEquals(480, AudioBufferPolicy.trackFloorFrames(33, AudioBufferPolicy.PERF_LOW_LATENCY, false, 240, 0, 192));
        // Android 7.x: a low latency request counts as granted.
        assertEquals(480, AudioBufferPolicy.trackFloorFrames(25, -1, true, 240, 1920, 192));

        AudioBufferPolicy policy = AudioBufferPolicy.forTrack(
                AudioBufferPolicy.trackStepFrames(true, 240, 192), 1920, 480, 480, 0, 0, AudioBufferPolicy.maxReserveFrames(48000));
        assertEquals(480, policy.sample(0, 1000));
        assertEquals(720, policy.sample(1, 2000));
        assertEquals(480, policy.sample(1, 32000));
        int frames = 0;
        for (int i = 2; i < 50; i++) frames = policy.sample(i, 32000 + i * 1000L);
        assertEquals("same eight-packet bound as before", 1920, frames);
    }

    // Retroid Pocket 6, 5.1 and 7.1: perfMode=none, framework minimum 3844 frames, capacity 5520 as measured.
    @Test public void surroundTrackOnTheNormalMixerNeverShrinksBelowItsMinimum() {
        assertEquals(3844, AudioBufferPolicy.trackFloorFrames(33, AudioBufferPolicy.PERF_NONE, true, 240, 3844, 192));
        assertEquals(5520, AudioBufferPolicy.trackCapacityFrames(240, 3844, 192));
        AudioBufferPolicy policy = AudioBufferPolicy.forTrack(
                AudioBufferPolicy.trackStepFrames(false, 240, 192), 5520, 3844, 3844, 0, 0, AudioBufferPolicy.maxReserveFrames(48000));
        assertEquals(3844, policy.sample(0, 1000));
        for (long t = 30000; t < 600000; t += 1000) assertEquals(3844, policy.sample(0, t));
        // Underruns still grow it, within the capacity.
        assertEquals(4084, policy.sample(3, 700000));
    }

    @Test public void growthStepFollowsTheOutputBurstOffTheFastPath() {
        assertEquals(240, AudioBufferPolicy.trackStepFrames(true, 240, 960));
        assertEquals(240, AudioBufferPolicy.trackStepFrames(false, 240, 0));
        assertEquals(480, AudioBufferPolicy.trackStepFrames(false, 240, 480));
    }

    @Test public void requestedCapacityHoldsTheFloorAndSixGrowthSteps() {
        assertEquals(1920, AudioBufferPolicy.trackCapacityFrames(240, 0, 0));
        assertEquals(1920, AudioBufferPolicy.trackCapacityFrames(240, 0, 192));
        assertEquals(2400, AudioBufferPolicy.trackCapacityFrames(240, 960, 192));
        assertEquals(4560, AudioBufferPolicy.trackCapacityFrames(240, 2886, 192));
        // A 960-frame burst still leaves six steps of 960 above the floor.
        int capacity = AudioBufferPolicy.trackCapacityFrames(240, 2886, 960);
        assertTrue(capacity >= 2886 + 6 * 960);
        assertEquals(0, capacity % 240);
    }

    @Test public void outputBurstPropertyIsParsedSafely() {
        assertEquals(192, AudioBufferPolicy.parseFrames("192"));
        assertEquals(192, AudioBufferPolicy.parseFrames(" 192 "));
        assertEquals(0, AudioBufferPolicy.parseFrames(null));
        assertEquals(0, AudioBufferPolicy.parseFrames(""));
        assertEquals(0, AudioBufferPolicy.parseFrames("abc"));
        assertEquals(0, AudioBufferPolicy.parseFrames("-5"));
    }

    @Test public void performanceModesHaveReportNames() {
        assertEquals("lowLatency", AudioBufferPolicy.perfModeName(AudioBufferPolicy.PERF_LOW_LATENCY));
        assertEquals("none", AudioBufferPolicy.perfModeName(AudioBufferPolicy.PERF_NONE));
        assertEquals("powerSaving", AudioBufferPolicy.perfModeName(AudioBufferPolicy.PERF_POWER_SAVING));
        assertEquals("unknown", AudioBufferPolicy.perfModeName(-1));
    }

    // TCL C6K TV (MediaTek MT9689, Android 14) on Wi-Fi: audio stalls for up to 130 ms every 1.2 s and then
    // arrives in one burst. Its AAudio stream gets the legacy path with an 864-frame (18 ms) burst.
    @Test public void aBurstAfterAStallRefillsTheReserveInsteadOfBeingDropped() {
        int tvReserve = 9 * 864;
        // 130 ms arrive at once and the 162 ms reserve has 30 ms left: all of it fits, nothing is late.
        assertFalse(AudioBufferPolicy.dropLate(130, 1440, tvReserve, 48000));
        // The old bound on the network queue alone dropped 90 ms of it at every stall.
        assertTrue(AudioBufferPolicy.dropLate(130, 0, 0, 48000));
        // Only what goes beyond the reserve plus 40 ms is late.
        assertFalse(AudioBufferPolicy.dropLate(170, 1440, tvReserve, 48000));
        assertTrue(AudioBufferPolicy.dropLate(175, 1440, tvReserve, 48000));
    }

    @Test public void aFullOutputKeepsTheOldFortyMillisecondBound() {
        // A phone's full 10 ms track or 4 ms ring: the network queue alone decides, as before.
        for (int reserve : new int[] {0, 192, 480, 7776}) {
            assertFalse(AudioBufferPolicy.dropLate(39, reserve, reserve, 48000));
            assertTrue(AudioBufferPolicy.dropLate(40, reserve, reserve, 48000));
        }
        // No output level at all, or no rate: the old bound.
        assertFalse(AudioBufferPolicy.dropLate(39, 0, 0, 0));
        assertTrue(AudioBufferPolicy.dropLate(40, 0, 0, 0));
        // An output holding more than its (just shrunk) reserve leaves less room for the network queue.
        assertTrue(AudioBufferPolicy.dropLate(30, 960, 480, 48000));
        assertFalse(AudioBufferPolicy.dropLate(29, 960, 480, 48000));
    }

    @Test public void aaudioReserveStaysWithinEightBurstsAndFortyMilliseconds() {
        // TV: 18 ms bursts. Eight (144 ms) would hold its 130 ms stalls but add their latency to every
        // stream; it keeps the two it starts with (36 ms) and a stall is concealed instead.
        AudioBufferPolicy tv = AudioBufferPolicy.forReserve(864, 16384, 1728, 0, 0, 48000);
        assertEquals(2 * 864, tv.limit());
        assertEquals(1728, tv.sample(0, 1000));
        for (int i = 1; i < 30; i++) assertEquals(1728, tv.sample(i, 1000 + i * 1000L));

        // Phones keep the eight bursts they had: a 4 ms burst (Retroid) up to 32 ms, a 2 ms one up to 16 ms.
        assertEquals(8 * 192, AudioBufferPolicy.forReserve(192, 3072, 384, 0, 0, 48000).limit());
        AudioBufferPolicy phone = AudioBufferPolicy.forReserve(96, 16384, 192, 0, 0, 48000);
        assertEquals(8 * 96, phone.limit());
        assertEquals(192, phone.sample(0, 1000));
        for (long t = 2000; t < 600000; t += 1000) assertEquals(192, phone.sample(0, t));
        assertEquals(288, phone.sample(1, 601000));

        // The ring bounds it, in whole bursts; a burst over 20 ms still gets its two.
        assertEquals(5 * 192, AudioBufferPolicy.forReserve(192, 1000, 384, 0, 0, 48000).limit());
        assertEquals(2 * 2400, AudioBufferPolicy.forReserve(2400, 38400, 4800, 0, 0, 48000).limit());
        assertEquals(1920, AudioBufferPolicy.maxReserveFrames(48000));
    }

    @Test public void aaudioReserveStartsWithAHostPeriodAndABurst() {
        // Retroid: 4 ms bursts. Two (8 ms) underran within a second of the first audio, as the host's
        // 5 ms packets come in pairs every 10 ms; it starts, and stays at least, at 10 ms plus a burst.
        int retroid = AudioBufferPolicy.reserveStartFrames(192, 48000);
        assertEquals(480 + 192, retroid);
        AudioBufferPolicy policy = AudioBufferPolicy.forReserve(192, 4096, retroid, 0, 0, 48000);
        assertEquals(672, policy.frames());
        assertEquals(8 * 192, policy.limit());
        assertEquals(864, policy.sample(1, 1000));
        for (long t = 2000; t < 120000; t += 1000) policy.sample(1, t);
        assertEquals(672, policy.frames());
        // A 2 ms burst gets the same 10 ms plus its burst; a TV's two 18 ms bursts already cover it.
        assertEquals(576, AudioBufferPolicy.reserveStartFrames(96, 48000));
        assertEquals(2 * 864, AudioBufferPolicy.reserveStartFrames(864, 48000));
        assertEquals(2 * 864, AudioBufferPolicy.forReserve(864, 16384,
                AudioBufferPolicy.reserveStartFrames(864, 48000), 0, 0, 48000).frames());
    }

    @Test public void aaudioReserveOfSmallBurstsGrowsAboveItsStart() {
        // 1 ms bursts start at 10 ms plus one: 528 frames, above eight bursts (384). Capped at eight, the
        // reserve could never grow and every jitter gap starved it at its limit.
        int start = AudioBufferPolicy.reserveStartFrames(48, 48000);
        assertEquals(528, start);
        AudioBufferPolicy policy = AudioBufferPolicy.forReserve(48, 4096, start, 0, 0, 48000);
        assertEquals(528, policy.frames());
        assertEquals(528 + 2 * 48, policy.limit());
        assertEquals(576, policy.sample(1, 1000));
        assertEquals(624, policy.sample(2, 2000));
        assertEquals(624, policy.sample(3, 3000));
        assertFalse(policy.needsFallback());
        // Still within 40 ms: a 10 ms burst starts at 20 ms and grows to 40 ms, not to 80.
        assertEquals(1920, AudioBufferPolicy.forReserve(480, 16384,
                AudioBufferPolicy.reserveStartFrames(480, 48000), 0, 0, 48000).limit());
    }

    @Test public void repeatingStallsAreReportedOnce() {
        // TCL C6K: a 130 ms stall every 1.2 s, read once a second: the third within a minute trips it.
        AudioBufferPolicy.StallWatch tv = new AudioBufferPolicy.StallWatch(0);
        assertFalse(tv.repeating(1, 1000));
        assertFalse(tv.repeating(2, 2000));
        assertTrue(tv.repeating(3, 3000));
        // Retroid: a Wi-Fi stall every 30 s or so is not a repeating problem.
        AudioBufferPolicy.StallWatch phone = new AudioBufferPolicy.StallWatch(4);
        assertFalse(phone.repeating(4, 1000));
        assertFalse(phone.repeating(5, 1000));
        assertFalse(phone.repeating(6, 31000));
        assertFalse(phone.repeating(7, 62000));
        assertFalse(phone.repeating(7, 63000));
        // Several between two samples count each.
        assertTrue(phone.repeating(9, 64000));
    }

    @Test public void fastTrackHoldsAtLeastOneOutputBurstAndAPacket() {
        // TV: the fast mixer takes 864 frames a cycle. A 480-frame buffer starved every cycle and the
        // stream lost 10% of its packets to the 40 ms bound.
        assertEquals(1104, AudioBufferPolicy.trackFloorFrames(34, AudioBufferPolicy.PERF_LOW_LATENCY, true, 240, 2928, 864));
        // Phones mix 192 or 240 frames: the two-packet floor stays.
        assertEquals(480, AudioBufferPolicy.trackFloorFrames(33, AudioBufferPolicy.PERF_LOW_LATENCY, true, 240, 2886, 240));
        assertEquals(480, AudioBufferPolicy.trackFloorFrames(33, AudioBufferPolicy.PERF_LOW_LATENCY, true, 240, 2886, 0));
        // The normal mixer's floor is the framework minimum, whatever the burst.
        assertEquals(2928, AudioBufferPolicy.trackFloorFrames(34, AudioBufferPolicy.PERF_NONE, true, 240, 2928, 864));
    }

    @Test public void trackBufferGrowsSixStepsAboveItsFloorOr40Milliseconds() {
        int max = AudioBufferPolicy.maxReserveFrames(48000);
        // TV: six steps above its 1104-frame floor (53 ms) are more than 40 ms.
        assertEquals(1104 + 6 * 240, AudioBufferPolicy.forTrack(240, 9840, 1104, 1104, 0, 0, max).limit());
        // A phone's 1920-frame fast track stays at its eight packets.
        assertEquals(1920, AudioBufferPolicy.forTrack(240, 1920, 480, 480, 0, 0, max).limit());
        // A small floor with room to spare may reach 40 ms.
        assertEquals(1920, AudioBufferPolicy.forTrack(120, 9840, 240, 240, 0, 0, max).limit());
        assertEquals(240 + 6 * 120, AudioBufferPolicy.forTrack(120, 9840, 240, 240, 0, 0, 0).limit());
    }

    @Test public void cleanPlaybackAndInvalidCountersClearTheFallbackHistory() {
        AudioBufferPolicy policy = new AudioBufferPolicy(192, 384, 384, 0, 0);
        policy.sample(20, 1000);
        assertFalse("A burst of underruns is one observation", policy.needsFallback());
        policy.sample(21, 2000);
        policy.sample(21, 32000);
        policy.sample(22, 33000);
        assertFalse(policy.needsFallback());
        policy.sample(-1, 34000);
        policy.sample(0, 35000);
        policy.sample(1, 36000);
        assertFalse(policy.needsFallback());
    }
}

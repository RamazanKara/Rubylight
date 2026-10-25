package com.limelight.binding.audio;

import org.junit.Test;
import static org.junit.Assert.*;

public class AudioBufferPolicyTest {
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
        int floor = AudioBufferPolicy.trackFloorFrames(33, AudioBufferPolicy.PERF_NONE, true, 240, ODIN_MIN_BUFFER);
        int step = AudioBufferPolicy.trackStepFrames(false, 240, 192);
        return AudioBufferPolicy.forTrack(step, ODIN_CAPACITY, floor, 480, 0, 0);
    }

    @Test public void deniedFastPathFloorsAtTheFrameworkMinimum() {
        assertEquals(2886, AudioBufferPolicy.trackFloorFrames(33, AudioBufferPolicy.PERF_NONE, true, 240, 2886));
        assertEquals(2886, AudioBufferPolicy.trackFloorFrames(33, AudioBufferPolicy.PERF_POWER_SAVING, false, 240, 2886));
        // A tiny platform minimum never drops below two packets.
        assertEquals(480, AudioBufferPolicy.trackFloorFrames(33, AudioBufferPolicy.PERF_NONE, false, 240, 100));
        // Android 7.x cannot report the mode: a plain track is a normal-mixer track.
        assertEquals(2886, AudioBufferPolicy.trackFloorFrames(25, -1, false, 240, 2886));
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
        // Six steps of growth above the floor, within the capacity.
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
        assertEquals(480, AudioBufferPolicy.trackFloorFrames(33, AudioBufferPolicy.PERF_LOW_LATENCY, true, 240, 2886));
        assertEquals(480, AudioBufferPolicy.trackFloorFrames(33, AudioBufferPolicy.PERF_LOW_LATENCY, false, 240, 0));
        // Android 7.x: a low latency request counts as granted.
        assertEquals(480, AudioBufferPolicy.trackFloorFrames(25, -1, true, 240, 1920));

        AudioBufferPolicy policy = AudioBufferPolicy.forTrack(
                AudioBufferPolicy.trackStepFrames(true, 240, 192), 1920, 480, 480, 0, 0);
        assertEquals(480, policy.sample(0, 1000));
        assertEquals(720, policy.sample(1, 2000));
        assertEquals(480, policy.sample(1, 32000));
        int frames = 0;
        for (int i = 2; i < 50; i++) frames = policy.sample(i, 32000 + i * 1000L);
        assertEquals("same eight-packet bound as before", 1920, frames);
    }

    // Retroid Pocket 6, 5.1 and 7.1: perfMode=none, framework minimum 3844 frames, capacity 5520 as measured.
    @Test public void surroundTrackOnTheNormalMixerNeverShrinksBelowItsMinimum() {
        assertEquals(3844, AudioBufferPolicy.trackFloorFrames(33, AudioBufferPolicy.PERF_NONE, true, 240, 3844));
        assertEquals(5520, AudioBufferPolicy.trackCapacityFrames(240, 3844, 192));
        AudioBufferPolicy policy = AudioBufferPolicy.forTrack(
                AudioBufferPolicy.trackStepFrames(false, 240, 192), 5520, 3844, 3844, 0, 0);
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

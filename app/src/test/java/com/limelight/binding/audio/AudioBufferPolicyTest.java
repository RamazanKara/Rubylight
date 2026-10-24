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

    // Reporter's Odin 2 Portal in #15: packet 240, framework minimum 1920 frames, capacity 2886, fast path denied.
    private static AudioBufferPolicy deniedFastPath() {
        int floor = AudioBufferPolicy.trackFloorFrames(33, AudioBufferPolicy.PERF_NONE, true, 240, 192, 1920);
        int step = AudioBufferPolicy.trackStepFrames(false, 240, 192);
        return AudioBufferPolicy.forTrack(step, 2886, floor, 480, 0, 0);
    }

    @Test public void deniedFastPathFloorsAtTheFrameworkMinimum() {
        assertEquals(1920, AudioBufferPolicy.trackFloorFrames(33, AudioBufferPolicy.PERF_NONE, true, 240, 192, 1920));
        assertEquals(1920, AudioBufferPolicy.trackFloorFrames(33, AudioBufferPolicy.PERF_POWER_SAVING, false, 240, 192, 1920));
        // A tiny platform minimum never drops below two packets.
        assertEquals(480, AudioBufferPolicy.trackFloorFrames(33, AudioBufferPolicy.PERF_NONE, false, 240, 0, 100));
        // Android 7.x cannot report the mode: a plain track is a normal-mixer track.
        assertEquals(1920, AudioBufferPolicy.trackFloorFrames(25, -1, false, 240, 192, 1920));
    }

    @Test public void deniedFastPathNeverShrinksBelowTheFloor() {
        AudioBufferPolicy policy = deniedFastPath();
        // The 480 frames requested in #15 are below the floor, and so is the 960 it later grew to.
        assertEquals(1920, policy.sample(0, 1000));
        for (long t = 30000; t < 600000; t += 1000) assertEquals(1920, policy.sample(0, t));
    }

    @Test public void deniedFastPathStillGrowsOnUnderrunsAndShrinksBackToTheFloor() {
        AudioBufferPolicy policy = deniedFastPath();
        assertEquals(1920, policy.sample(0, 1000));
        assertEquals(2160, policy.sample(5, 2000));
        assertEquals(2400, policy.sample(9, 3000));
        // Growth stops at the capacity.
        int frames = 0;
        for (int i = 1; i < 50; i++) frames = policy.sample(10 + i, 3000 + i * 1000L);
        assertEquals(2886, frames);
        policy.applied(2400);
        assertEquals(2400, policy.sample(59, 60000));
        assertEquals(2160, policy.sample(59, 83000));
        assertEquals(1920, policy.sample(59, 114000));
        assertEquals(1920, policy.sample(59, 145000));
    }

    @Test public void fastTrackKeepsTheSmallFloor() {
        assertEquals(480, AudioBufferPolicy.trackFloorFrames(33, AudioBufferPolicy.PERF_LOW_LATENCY, true, 240, 0, 1920));
        assertEquals(480, AudioBufferPolicy.trackFloorFrames(33, AudioBufferPolicy.PERF_LOW_LATENCY, true, 240, 192, 1920));
        assertEquals(960, AudioBufferPolicy.trackFloorFrames(33, AudioBufferPolicy.PERF_LOW_LATENCY, true, 240, 480, 1920));
        // Android 7.x: a low latency request counts as granted.
        assertEquals(480, AudioBufferPolicy.trackFloorFrames(25, -1, true, 240, 0, 1920));

        int floor = AudioBufferPolicy.trackFloorFrames(33, AudioBufferPolicy.PERF_LOW_LATENCY, true, 240, 192, 1920);
        AudioBufferPolicy policy = AudioBufferPolicy.forTrack(
                AudioBufferPolicy.trackStepFrames(true, 240, 192), 1920, floor, 480, 0, 0);
        assertEquals(480, policy.sample(0, 1000));
        assertEquals(720, policy.sample(1, 2000));
        assertEquals(480, policy.sample(1, 32000));
        int frames = 0;
        for (int i = 2; i < 50; i++) frames = policy.sample(i, 32000 + i * 1000L);
        assertEquals("same eight-packet bound as before", 1920, frames);
    }

    @Test public void growthStepFollowsTheOutputBurstOffTheFastPath() {
        assertEquals(240, AudioBufferPolicy.trackStepFrames(true, 240, 960));
        assertEquals(240, AudioBufferPolicy.trackStepFrames(false, 240, 0));
        assertEquals(480, AudioBufferPolicy.trackStepFrames(false, 240, 480));
    }

    @Test public void requestedCapacityHoldsTheFloorAndHeadroom() {
        // Unchanged for devices with a small platform minimum.
        assertEquals(1920, AudioBufferPolicy.trackCapacityFrames(240, 960));
        assertEquals(1920, AudioBufferPolicy.trackCapacityFrames(240, 0));
        assertEquals(2880, AudioBufferPolicy.trackCapacityFrames(240, 1920));
        assertEquals(2880, AudioBufferPolicy.trackCapacityFrames(240, 1850));
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

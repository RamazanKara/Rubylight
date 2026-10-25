package com.limelight.binding.video;

import org.junit.Test;

import static com.limelight.binding.video.UpscalingPolicy.Mode.*;
import static com.limelight.binding.video.UpscalingPolicy.Reason.*;
import static org.junit.Assert.*;

public class UpscalingPolicyTest {
    @Test
    public void onlySmallerSdrMediaCodecStreamsUseTheGpu() {
        assertEquals(DISABLED, decision(OFF, 1280, 720, 1920, 1080, false, false, true));
        for (UpscalingPolicy.Mode mode : new UpscalingPolicy.Mode[] {BILINEAR, FSR1, SGSR1}) {
            assertEquals(NONE, decision(mode, 1280, 720, 1920, 1080, false, false, true));
            assertEquals(NONE, decision(mode, 1920, 1080, 2560, 1440, false, false, true));
            assertEquals(HDR, decision(mode, 1280, 720, 1920, 1080, true, false, true));
            assertEquals(PYROWAVE, decision(mode, 1280, 720, 1920, 1080, false, true, true));
            assertEquals(UNSUPPORTED, decision(mode, 1280, 720, 1920, 1080, false, false, false));
            assertEquals(NO_UPSCALE, decision(mode, 1920, 1080, 1920, 1080, false, false, true));
            assertEquals(NO_UPSCALE, decision(mode, 2560, 1440, 1920, 1080, false, false, true));
            assertEquals(NO_UPSCALE, decision(mode, 1920, 1080, 2560, 720, false, false, true));
        }
        assertEquals(OFF, UpscalingPolicy.Mode.fromPreference("unknown"));
        assertEquals(OFF, UpscalingPolicy.Mode.fromPreference(null));
        assertEquals(FSR1, UpscalingPolicy.Mode.fromPreference("fsr1"));
        assertEquals(SGSR1, UpscalingPolicy.Mode.fromPreference("sgsr1"));
    }

    @Test
    public void tenBitSdrIsNamedAsTenBitAndRealHdrStaysHdr() {
        assertEquals(TEN_BIT, UpscalingPolicy.colorReason(HDR, false, true));
        assertEquals(HDR, UpscalingPolicy.colorReason(HDR, true, true));
        assertEquals(HDR, UpscalingPolicy.colorReason(HDR, false, false));
        assertEquals(NONE, UpscalingPolicy.colorReason(NONE, false, true));
        assertEquals(PYROWAVE, UpscalingPolicy.colorReason(PYROWAVE, false, true));
    }

    private UpscalingPolicy.Reason decision(UpscalingPolicy.Mode mode, int width, int height,
                                            int outputWidth, int outputHeight, boolean hdr,
                                            boolean pyroWave, boolean supported) {
        return UpscalingPolicy.unavailableReason(mode, width, height, outputWidth, outputHeight, hdr, pyroWave, supported);
    }

    @Test
    public void scalingUsesTheLetterboxedViewportAndSupportsPortraitAndStretch() {
        assertArrayEquals(new int[] {240, 0, 1920, 1080}, UpscalingPolicy.viewport(1280, 720, 2400, 1080, false));
        assertArrayEquals(new int[] {0, 0, 2400, 1080}, UpscalingPolicy.viewport(1280, 720, 2400, 1080, true));
        assertArrayEquals(new int[] {0, 240, 1080, 1920}, UpscalingPolicy.viewport(720, 1280, 1080, 2400, false));
        assertArrayEquals(new int[] {0, 80, 2560, 1440}, UpscalingPolicy.viewport(1920, 1080, 2560, 1600, false));
        assertThrows(IllegalArgumentException.class, () -> UpscalingPolicy.viewport(1280, 720, 0, 1080, false));
    }

    private UpscalingPolicy warmedPolicy(int fps) {
        UpscalingPolicy policy = new UpscalingPolicy(fps);
        for (int i = 0; i < UpscalingPolicy.WARMUP_FRAMES; i++) policy.recordFrame(5000000, 0);
        return policy;
    }

    /** The frames in the policy's window at this rate. */
    private static int window(int fps) {
        return (int) Math.max(UpscalingPolicy.MIN_WINDOW_FRAMES, UpscalingPolicy.WINDOW_NS / (1000000000L / fps));
    }

    @Test
    public void startupAndIsolatedSpikesDoNotDisableUpscaling() {
        UpscalingPolicy policy = new UpscalingPolicy(60);
        for (int i = 0; i < UpscalingPolicy.WARMUP_FRAMES; i++) policy.recordFrame(40000000, 1);
        assertEquals(NONE, policy.getReason());
        for (int i = 0; i < 200; i++) {
            policy.recordFrame(20000000, 0);
            policy.recordFrame(4000000, 0);
        }
        assertEquals(NONE, policy.getReason());
        assertTrue(policy.getAddedMs() > 0);
    }

    @Test
    public void spikesSpreadOverMinutesNeverAddUp() {
        // The tester's stream: a slow frame now and then for minutes, at 120 fps. The old counter
        // only took one slow sample off for a frame well under budget and ended the stream.
        UpscalingPolicy policy = warmedPolicy(120);
        for (int i = 0; i < 120 * 60 * 5; i++) {
            policy.recordFrame(i % 40 == 0 ? 12000000 : 3000000, 0);
        }
        assertEquals(NONE, policy.getReason());
    }

    @Test
    public void aBurstShorterThanTheWindowDoesNotFail() {
        for (int fps : new int[] {30, 60, 120, 240}) {
            UpscalingPolicy policy = warmedPolicy(fps);
            for (int i = 0; i < window(fps); i++) policy.recordFrame(3000000, 0);
            // Up to half a window over budget in a row is a hiccup, not overload.
            for (int i = 0; i < window(fps) / 2; i++) policy.recordFrame(30000000, 0);
            for (int i = 0; i < window(fps); i++) policy.recordFrame(3000000, 0);
            assertEquals("fps " + fps, NONE, policy.getReason());
        }
    }

    @Test
    public void sustainedOverloadLatchesFallbackUntilReconnect() {
        for (int fps : new int[] {30, 60, 120, 240}) {
            UpscalingPolicy policy = warmedPolicy(fps);
            long over = 1000000000L / fps + 2000000;
            // Not judged before a window of samples exists.
            for (int i = 0; i < window(fps) - 1; i++) policy.recordFrame(over, 0);
            assertEquals("fps " + fps, NONE, policy.getReason());
            policy.recordFrame(over, 0);
            assertEquals("fps " + fps, SLOW, policy.getReason());
            for (int i = 0; i < 1000; i++) policy.recordFrame(1000000, 0);
            assertEquals("fps " + fps, SLOW, policy.getReason());
        }
        assertEquals(NONE, warmedPolicy(60).getReason());
    }

    @Test
    public void overloadOnMostButNotAllFramesStillFails() {
        UpscalingPolicy policy = warmedPolicy(60);
        // Two frames in three over budget, steadily.
        for (int i = 0; i < 3 * window(60) && policy.getReason() == NONE; i++) {
            policy.recordFrame(i % 3 == 0 ? 5000000 : 25000000, 0);
        }
        assertEquals(SLOW, policy.getReason());
    }

    @Test
    public void exactlyHalfOverBudgetIsNotMostly() {
        UpscalingPolicy policy = warmedPolicy(60);
        for (int i = 0; i < 10 * window(60); i++) policy.recordFrame(i % 2 == 0 ? 5000000 : 25000000, 0);
        assertEquals(NONE, policy.getReason());
    }

    @Test
    public void budgetTracksFrameRateAndUsesAStrictOneFrameThreshold() {
        UpscalingPolicy sixty = warmedPolicy(60);
        UpscalingPolicy oneTwenty = warmedPolicy(120);
        for (int i = 0; i < 2 * window(120); i++) {
            sixty.recordFrame(16666666, 0);
            oneTwenty.recordFrame(10000000, 0);
        }
        assertEquals(NONE, sixty.getReason());
        assertEquals(SLOW, oneTwenty.getReason());
    }

    @Test
    public void frameRateChangesRebudgetExistingUpscalingWithoutRevivingFallback() {
        UpscalingPolicy policy = warmedPolicy(120);
        for (int i = 0; i < window(120) - 1; i++) policy.recordFrame(10000000, 0);
        policy.setFrameRate(60);
        // The window restarts with the new budget: 10 ms and 15 ms fit 60 fps.
        for (int i = 0; i < 2 * window(60); i++) policy.recordFrame(i % 2 == 0 ? 10000000 : 15000000, 0);
        assertEquals(NONE, policy.getReason());
        policy.setFrameRate(120);
        for (int i = 0; i < 2 * window(120); i++) policy.recordFrame(10000000, 0);
        assertEquals(SLOW, policy.getReason());
        policy.setFrameRate(60);
        assertEquals(SLOW, policy.getReason());
    }

    @Test
    public void coalescedFramesDoNotDisableAnUpscalerThatMeetsItsBudget() {
        UpscalingPolicy policy = warmedPolicy(60);
        for (int i = 0; i < 4 * window(60); i++) policy.recordFrame(4000000, 3);
        assertEquals(NONE, policy.getReason());
        policy.fail(SLOW);
        assertEquals(SLOW, policy.getReason());
    }

    @Test
    public void aGpuThatStopsCompletingFramesAlsoFallsBackButAShortGapDoesNot() {
        for (int fps : new int[] {60, 120, 240}) {
            UpscalingPolicy policy = warmedPolicy(fps);
            // Six frames at 120 fps (50 ms) failed the stream before; a gap that short is a hiccup.
            policy.checkStall(100000000L);
            policy.checkStall(UpscalingPolicy.STALL_LIMIT_NS);
            assertEquals("fps " + fps, NONE, policy.getReason());
            policy.checkStall(UpscalingPolicy.STALL_LIMIT_NS + 1);
            assertEquals("fps " + fps, SLOW, policy.getReason());
        }
        assertTrue("a stall limit under 500 ms would fail on routine hiccups",
                UpscalingPolicy.STALL_LIMIT_NS >= 500000000L);
    }
}

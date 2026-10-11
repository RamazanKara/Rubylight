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
    public void sustainedOverBudgetFramesLatchFallbackUntilReconnect() {
        UpscalingPolicy policy = warmedPolicy(60);
        for (int i = 1; i < UpscalingPolicy.SLOW_FRAME_LIMIT; i++) policy.recordFrame(17000000, 0);
        assertEquals(NONE, policy.getReason());
        // The hysteresis band must not erase slow samples.
        policy.recordFrame(15000000, 0);
        policy.recordFrame(17000000, 0);
        assertEquals(SLOW, policy.getReason());
        for (int i = 0; i < 1000; i++) policy.recordFrame(1000000, 0);
        assertEquals(SLOW, policy.getReason());
        assertEquals(NONE, warmedPolicy(60).getReason());
    }

    @Test
    public void budgetTracksFrameRateAndUsesAStrictOneFrameThreshold() {
        UpscalingPolicy sixty = warmedPolicy(60);
        UpscalingPolicy oneTwenty = warmedPolicy(120);
        for (int i = 0; i < UpscalingPolicy.SLOW_FRAME_LIMIT; i++) {
            sixty.recordFrame(16666666, 0);
            oneTwenty.recordFrame(10000000, 0);
        }
        assertEquals(NONE, sixty.getReason());
        assertEquals(SLOW, oneTwenty.getReason());
    }

    @Test
    public void frameRateChangesRebudgetExistingUpscalingWithoutRevivingFallback() {
        UpscalingPolicy policy = warmedPolicy(120);
        for (int i = 1; i < UpscalingPolicy.SLOW_FRAME_LIMIT; i++) policy.recordFrame(10000000, 0);
        policy.setFrameRate(60);
        policy.recordFrame(15000000, 0);
        policy.recordFrame(20000000, 0);
        assertEquals(NONE, policy.getReason());
        for (int i = 0; i < UpscalingPolicy.SLOW_FRAME_LIMIT; i++) policy.recordFrame(10000000, 0);
        assertEquals(NONE, policy.getReason());
        policy.setFrameRate(120);
        for (int i = 0; i < UpscalingPolicy.SLOW_FRAME_LIMIT; i++) policy.recordFrame(10000000, 0);
        assertEquals(SLOW, policy.getReason());
        policy.setFrameRate(60);
        assertEquals(SLOW, policy.getReason());
    }

    @Test
    public void frameRateChangeAlsoUpdatesTheGpuStallDeadline() {
        UpscalingPolicy policy = warmedPolicy(120);
        policy.setFrameRate(60);
        policy.checkStall(90000000);
        assertEquals(NONE, policy.getReason());
        policy.setFrameRate(120);
        policy.checkStall(90000000);
        assertEquals(SLOW, policy.getReason());
    }

    @Test
    public void coalescedFramesDoNotDisableAnUpscalerThatMeetsItsBudget() {
        UpscalingPolicy policy = warmedPolicy(60);
        for (int i = 0; i < 120; i++) policy.recordFrame(4000000, 3);
        assertEquals(NONE, policy.getReason());
        policy.fail(SLOW);
        assertEquals(SLOW, policy.getReason());
    }

    @Test
    public void aGpuThatStopsCompletingFramesAlsoFallsBack() {
        UpscalingPolicy policy = warmedPolicy(60);
        policy.checkStall(90000000);
        assertEquals(NONE, policy.getReason());
        policy.checkStall(110000000);
        assertEquals(SLOW, policy.getReason());
    }
}

package com.limelight.binding.video;

import org.junit.Test;

import static org.junit.Assert.assertEquals;

/** A frame on time waits nothing; one right behind the previous input waits up to half a frame. */
public class InputSpacerTest {
    private static final int FPS_60 = 60_000;
    private static final long HALF_FRAME_60 = FramePacingPolicy.frameIntervalNs(FPS_60) / 2;

    @Test
    public void theFirstFrameWaitsNothing() {
        assertEquals(0, new InputSpacer().holdNs(1_000_000, FPS_60));
    }

    @Test
    public void aFrameOnTimeWaitsNothing() {
        InputSpacer spacer = new InputSpacer();
        spacer.onInput(0);
        assertEquals(0, spacer.holdNs(16_666_667, FPS_60));
    }

    @Test
    public void aFrameTwoMillisecondsEarlyWaitsNothing() {
        InputSpacer spacer = new InputSpacer();
        spacer.onInput(0);
        assertEquals(0, spacer.holdNs(14_000_000, FPS_60));
    }

    @Test
    public void exactlyHalfAFrameBehindWaitsNothing() {
        InputSpacer spacer = new InputSpacer();
        spacer.onInput(0);
        assertEquals(0, spacer.holdNs(HALF_FRAME_60, FPS_60));
    }

    @Test
    public void backToBackWaitsHalfAFrame() {
        InputSpacer spacer = new InputSpacer();
        spacer.onInput(1_000);
        assertEquals(HALF_FRAME_60, spacer.holdNs(1_000, FPS_60));
    }

    @Test
    public void partWayWaitsTheRest() {
        InputSpacer spacer = new InputSpacer();
        spacer.onInput(0);
        assertEquals(HALF_FRAME_60 - 3_000_000, spacer.holdNs(3_000_000, FPS_60));
    }

    @Test
    public void aLowFrameRateWaitsNoLongerThanTheCap() {
        InputSpacer spacer = new InputSpacer();
        spacer.onInput(0);
        assertEquals(InputSpacer.MAX_HOLD_NS, spacer.holdNs(0, 1_000));
    }

    @Test
    public void anUnknownFrameRateWaitsNothing() {
        InputSpacer spacer = new InputSpacer();
        spacer.onInput(0);
        assertEquals(0, spacer.holdNs(0, 0));
    }

    @Test
    public void aHighFrameRateSpacesLess() {
        InputSpacer spacer = new InputSpacer();
        spacer.onInput(0);
        assertEquals(FramePacingPolicy.frameIntervalNs(120_000) / 2, spacer.holdNs(0, 120_000));
    }
}

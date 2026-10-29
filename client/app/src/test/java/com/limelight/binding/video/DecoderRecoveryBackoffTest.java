package com.limelight.binding.video;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

public class DecoderRecoveryBackoffTest {
    @Test
    public void recoveriesWithoutAFrameWaitLongerAndThenGiveUp() {
        // A TV's HEVC decoder out of memory failed ten resets in 2 s before showing anything.
        DecoderRecoveryBackoff backoff = new DecoderRecoveryBackoff();
        assertEquals(0, backoff.beforeRecovery(0));
        assertEquals(250, backoff.beforeRecovery(0));
        assertEquals(500, backoff.beforeRecovery(0));
        assertEquals(1000, backoff.beforeRecovery(0));
        assertEquals(2000, backoff.beforeRecovery(0));
        assertEquals(5, backoff.recoveriesWithoutFrame());
        assertEquals(-1, backoff.beforeRecovery(0));
        assertEquals(6, backoff.recoveriesWithoutFrame());
    }

    @Test
    public void aFrameBetweenRecoveriesStartsCountingAgain() {
        DecoderRecoveryBackoff backoff = new DecoderRecoveryBackoff();
        assertEquals(0, backoff.beforeRecovery(0));
        assertEquals(250, backoff.beforeRecovery(0));
        // The decoder recovered and put out frames: the next failure is a fresh one.
        assertEquals(0, backoff.beforeRecovery(120));
        assertEquals(1, backoff.recoveriesWithoutFrame());
        assertEquals(250, backoff.beforeRecovery(120));
        assertEquals(0, backoff.beforeRecovery(121));
    }

    @Test
    public void failuresWhileFramesFlowNeverGiveUp() {
        // A decoder that fails now and then mid-stream is left to the overall recovery limit.
        DecoderRecoveryBackoff backoff = new DecoderRecoveryBackoff();
        for (int frames = 1; frames <= 50; frames++) {
            assertEquals(0, backoff.beforeRecovery(frames * 60));
        }
    }
}

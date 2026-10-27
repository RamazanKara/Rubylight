package com.limelight.binding.video;

import org.junit.Test;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

public class LatencyProbeTest {
    @Test
    public void firstFrameReceivedAfterTheNudgeIsTimedToItsRender() throws InterruptedException {
        LatencyProbe probe = new LatencyProbe();
        probe.onFrameReceived(1_000_000_000L);
        assertFalse(probe.isIdle(1_100_000_000L));
        assertTrue(probe.isIdle(1_200_000_000L));
        probe.arm(1_200_000_000L);
        // A frame that left the host before the nudge doesn't count.
        probe.onFrameRendered(1_150_000_000L, 1_210_000_000L);
        probe.onFrameRendered(1_220_000_000L, 1_230_000_000L);
        probe.onFrameRendered(1_240_000_000L, 1_250_000_000L);
        assertEquals(30_000_000L, probe.await(0));
        assertEquals(-1, probe.await(0));
    }

    @Test
    public void summaryIsBestMedianAndP90() {
        long[] samples = new long[10];
        for (int i = 0; i < 10; i++) samples[i] = (10 - i) * 1_000_000L;
        assertArrayEquals(new float[] {1, 6, 9}, LatencyProbe.summarize(samples, 10), 0.001f);
        assertNull(LatencyProbe.summarize(samples, 0));
    }
}

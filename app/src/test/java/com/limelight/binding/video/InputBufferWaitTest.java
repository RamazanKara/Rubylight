package com.limelight.binding.video;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public class InputBufferWaitTest {
    @Test
    public void aPrefetchAsksOnceWithoutWaiting() {
        assertEquals(0, InputBufferWait.timeoutUs(true));
        assertTrue(InputBufferWait.askAgain(true, 0, 0));
        assertFalse("one try only", InputBufferWait.askAgain(true, 1, 0));
        assertFalse(InputBufferWait.askAgain(true, 1, InputBufferWait.HANG_MS));
    }

    @Test
    public void aFramesFetchWaitsInStepsUntilTheHangLimit() {
        assertEquals(1000, InputBufferWait.timeoutUs(false));
        assertTrue(InputBufferWait.askAgain(false, 0, 0));
        assertTrue(InputBufferWait.askAgain(false, 1, 1));
        assertTrue(InputBufferWait.askAgain(false, 4000, InputBufferWait.HANG_MS - 1));
        assertFalse("hung", InputBufferWait.askAgain(false, 5000, InputBufferWait.HANG_MS));
        // A fetch that starts late (a slow first call) still tries once.
        assertTrue(InputBufferWait.askAgain(false, 0, InputBufferWait.HANG_MS + 10));
    }

    @Test
    public void onlyAFramesFetchFailsWithoutABuffer() {
        assertTrue("stall count, hang detection, key frame", InputBufferWait.missingBufferFails(false));
        assertFalse("the next frame's fetch waits instead", InputBufferWait.missingBufferFails(true));
    }

    /** The loop as the renderer runs it, against a decoder whose buffers free up at given times. */
    private static int[] fetch(boolean prefetch, long freeAtMs) {
        long nowMs = 0;
        int attempts = 0;
        int index = -1;
        while (index < 0 && InputBufferWait.askAgain(prefetch, attempts, nowMs)) {
            long timeoutMs = InputBufferWait.timeoutUs(prefetch) / 1000;
            if (nowMs + timeoutMs >= freeAtMs) {
                nowMs = Math.max(nowMs, freeAtMs);
                index = 3;
            } else {
                nowMs += timeoutMs;
            }
            attempts++;
        }
        return new int[]{index, (int) nowMs, attempts};
    }

    @Test
    public void thePrefetchNeverHoldsTheThreadAndTheFrameWaitsInstead() {
        // A free buffer: both take it at once.
        assertEquals(3, fetch(true, 0)[0]);
        assertEquals(3, fetch(false, 0)[0]);
        // Buffers still busy for 15 ms: the prefetch returns at once without one...
        int[] prefetch = fetch(true, 15);
        assertEquals(-1, prefetch[0]);
        assertEquals(0, prefetch[1]);
        assertEquals(1, prefetch[2]);
        // ...and the frame's fetch waits the 15 ms.
        int[] frame = fetch(false, 15);
        assertEquals(3, frame[0]);
        assertEquals(15, frame[1]);
        // A decoder that never frees one: the frame's fetch gives up at the hang limit.
        int[] hung = fetch(false, Long.MAX_VALUE);
        assertEquals(-1, hung[0]);
        assertEquals(InputBufferWait.HANG_MS, hung[1]);
    }
}

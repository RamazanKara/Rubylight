package com.limelight.binding.video;

import org.junit.Test;
import static org.junit.Assert.*;

public class UpscalingFrameQueueTest {
    @Test
    public void coalescedCallbacksCountOnlyLocallyReleasedFrames() {
        UpscalingFrameQueue queue = new UpscalingFrameQueue();
        long[] frame = new long[3];
        queue.release(100, 150);
        queue.release(200, 250);
        long newest = queue.release(300, 350);
        assertTrue(queue.consume(newest, frame));
        assertArrayEquals(new long[] {300, 350, 2}, frame);
        assertFalse(queue.consume(newest, frame));
        assertFalse(queue.consume(200, frame));
        assertEquals(0, queue.oldestReleaseNs());
        assertTrue(queue.consume(queue.release(400, 0), frame));
        assertArrayEquals(new long[] {400, 0, 0}, frame);
    }

    @Test
    public void timestampTokensAreUniqueAndOverflowStillCountsLostFrames() {
        UpscalingFrameQueue queue = new UpscalingFrameQueue();
        long[] frame = new long[3];
        long last = 0;
        for (int i = 0; i < 100; i++) {
            long timestamp = queue.release(1000, 0);
            assertTrue(timestamp > last);
            last = timestamp;
        }
        assertTrue(queue.consume(last, frame));
        assertEquals(99, frame[2]);
        assertEquals(0, queue.oldestReleaseNs());
    }

    @Test
    public void theBoundedQueueWrapsWithoutInventingDropsOrLosingDeadlines() {
        UpscalingFrameQueue queue = new UpscalingFrameQueue();
        long[] frame = new long[3];
        for (int i = 1; i < 10000; i++) {
            long timestamp = queue.release(i * 1000L, i * 1000L + 500);
            assertEquals(timestamp, queue.oldestReleaseNs());
            assertTrue(queue.consume(timestamp, frame));
            assertArrayEquals(new long[] {timestamp, i * 1000L + 500, 0}, frame);
        }
    }
}

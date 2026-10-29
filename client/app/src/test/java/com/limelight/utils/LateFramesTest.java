package com.limelight.utils;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

/** The same cases as nightjar-policy's a_frame_is_late_when_it_arrives_later_than_the_host_sent_it. */
public class LateFramesTest {
    @Test public void aFrameIsLateWhenItArrivesLaterThanTheHostSentIt() {
        LateFrames late = new LateFrames();
        // 60 fps. Arrivals on time, then a 40 ms hold and two frames back to back behind it.
        long[][] frames = {{0, 0}, {16_667, 16_700}, {33_333, 33_300}, {50_000, 73_300}, {66_667, 73_900}, {83_333, 74_500}};
        for (long[] frame : frames) late.push(frame[0], frame[1], 60);
        assertEquals("only the frame after the hold is late", 1, late.tick());
        assertEquals("a tick starts the count over", 0, late.tick());
        // A still desktop or a capture stall: the timestamps step as far as the arrivals.
        late.push(183_333, 174_500, 60);
        // A timestamp from before the last one is not judged.
        late.push(100_000, 180_000, 60);
        late.push(116_667, 196_700, 60);
        assertEquals(0, late.tick());
        // A new stream's clock is not judged either.
        late.push(20_000_000, 200_000, 60);
        late.push(20_016_667, 400_000, 60);
        late.push(30_000_000, 900_000, 60);
        assertEquals("only the 183 ms hold of one frame interval", 1, late.tick());
        assertEquals(12_500, LateFrames.thresholdUs(60));
        assertEquals(8_000, LateFrames.thresholdUs(120));
        assertEquals(Long.MAX_VALUE, LateFrames.thresholdUs(0));
    }
}

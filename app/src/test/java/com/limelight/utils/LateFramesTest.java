package com.limelight.utils;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

/** The same cases as nightjar-policy's a_frame_is_late_when_it_arrives_later_than_the_host_sent_it. */
public class LateFramesTest {
    @Test public void aFrameIsLateWhenItArrivesLaterThanTheHostSentIt() {
        LateFrames late = new LateFrames();
        // 60 fps. Arrivals on time, then a 40 ms hold and two frames back to back behind it.
        long[][] frames = {{0, 0}, {16_667, 16_700}, {33_333, 33_300}, {50_000, 73_300}, {66_667, 73_900}, {83_333, 74_500}};
        for (long[] frame : frames) late.push(frame[0], 0, frame[1], 60);
        assertEquals("only the frame after the hold is late", 1, late.tick());
        assertEquals("a tick starts the count over", 0, late.tick());
        // A still desktop or a capture stall: the timestamps step as far as the arrivals.
        late.push(183_333, 0, 174_500, 60);
        // A timestamp from before the last one is not judged.
        late.push(100_000, 0, 180_000, 60);
        late.push(116_667, 0, 196_700, 60);
        assertEquals(0, late.tick());
        // A new stream's clock is not judged either.
        late.push(20_000_000, 0, 200_000, 60);
        late.push(20_016_667, 0, 400_000, 60);
        late.push(30_000_000, 0, 900_000, 60);
        assertEquals("only the 183 ms hold of one frame interval", 1, late.tick());
        assertEquals(12_500, LateFrames.thresholdUs(60));
        assertEquals(8_000, LateFrames.thresholdUs(120));
        assertEquals(Long.MAX_VALUE, LateFrames.thresholdUs(0));
    }

    /** As nightjar-policy's a_frame_the_host_took_longer_to_process_is_not_late. */
    @Test public void aFrameTheHostTookLongerToProcessIsNotLate() {
        // 60 fps on a clean network, but the host's processing time jumps from 3 to 40 ms: that frame
        // arrives 37 ms after its step, the two behind it waited for it and come right after.
        long[][] frames = {{0, 3_000, 3_000}, {16_667, 3_000, 19_700}, {33_333, 40_000, 73_300},
                {50_000, 24_000, 74_000}, {66_667, 8_000, 74_700}, {83_333, 3_000, 86_300}};
        LateFrames late = new LateFrames();
        for (long[] frame : frames) late.push(frame[0], frame[1], frame[2], 60);
        assertEquals("the host's processing spike alone is no late frame", 0, late.tick());
        // The same arrivals without a processing time are judged by the timestamps alone.
        late = new LateFrames();
        for (long[] frame : frames) late.push(frame[0], 0, frame[2], 60);
        assertEquals(1, late.tick());
        // A network hold on top of a faster frame still counts: 40 ms late, 5 ms quicker to process.
        late = new LateFrames();
        late.push(222_222, 8_000, 300_000, 60);
        late.push(238_889, 3_000, 356_700, 60);
        assertEquals(1, late.tick());
    }
}

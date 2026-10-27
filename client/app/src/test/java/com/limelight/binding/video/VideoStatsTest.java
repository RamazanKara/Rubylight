package com.limelight.binding.video;

import org.junit.Test;

import static org.junit.Assert.assertEquals;

public class VideoStatsTest {
    @Test
    public void missingHostTimingDoesNotEraseObservedMinimum() {
        VideoStats stats = new VideoStats();
        stats.minHostProcessingLatency = 50;
        stats.add(new VideoStats());
        assertEquals(50, stats.minHostProcessingLatency);
    }

    @Test
    public void accumulatedHostTimingDoesNotOverflowAnInteger() {
        VideoStats stats = new VideoStats();
        VideoStats window = new VideoStats();
        window.totalHostProcessingLatency = 1000000;
        for (int i = 0; i < 3000; i++) {
            stats.add(window);
        }
        assertEquals(3000000000L, stats.totalHostProcessingLatency);
    }

    @Test
    public void decodeTimeFollowsTheWindowsLikeArtemis() {
        VideoStats last = new VideoStats();
        last.decoderTimeMs = 240;
        last.totalFramesReceived = 60;
        VideoStats active = new VideoStats();
        active.decoderTimeMs = 180;
        active.totalFramesReceived = 60;
        VideoStats lastTwo = new VideoStats();
        lastTwo.add(last);
        lastTwo.add(active);
        assertEquals(3.5f, (float) lastTwo.decoderTimeMs / lastTwo.totalFramesReceived, 0);
        last.copy(active);
        assertEquals(180, last.decoderTimeMs);
        active.clear();
        assertEquals(0, active.decoderTimeMs);
    }

    @Test
    public void postStreamDecodeTimeIncludesUnfinishedWindowAndUnflushedSamples() {
        VideoStats completed = new VideoStats();
        VideoStats active = new VideoStats();
        assertEquals(0, completed.getAverageDecoderLatencyMs(active, 0));

        active.totalFramesReceived = 2;
        assertEquals(4, completed.getAverageDecoderLatencyMs(active, 8));

        completed.totalFramesReceived = 2;
        completed.decoderTimeMs = 12;
        active.decoderTimeMs = 4;
        assertEquals(6, completed.getAverageDecoderLatencyMs(active, 8));
    }

    @Test
    public void pyrowavePostStreamTimeMatchesMoonlightXCompletedWindowsOnly() {
        VideoStats global = new VideoStats();
        VideoStats active = new VideoStats();
        active.totalFramesReceived = 2;
        active.decoderTimeMs = 7;
        // Like X, a stream shorter than its first completed window reports zero.
        assertEquals(0, global.getAverageDecoderLatencyMs());
        global.add(active);
        active.clear();
        active.totalFramesReceived = 2;
        active.decoderTimeMs = 1;
        assertEquals(3, global.getAverageDecoderLatencyMs());
        // Conventional codecs keep Nightjar's existing current-window accounting.
        assertEquals(2, global.getAverageDecoderLatencyMs(active, 0));
    }

    @Test
    public void measuredZeroSubmissionAndMissingConventionalDecodeStayDistinct() {
        VideoStats window = new VideoStats();
        assertEquals(-1, window.getWindowDecoderLatencyMs(true), 0);
        window.totalFramesReceived = 1;
        assertEquals(0, window.getWindowDecoderLatencyMs(true), 0);
        assertEquals(-1, window.getWindowDecoderLatencyMs(false), 0);
        window.decoderTimeMs = 2;
        assertEquals(2, window.getWindowDecoderLatencyMs(false), 0);
    }
}

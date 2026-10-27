package com.limelight.binding.video;

import org.junit.Test;

import static org.junit.Assert.assertEquals;

public class PyroWaveSubmissionTimingTest {
    @Test
    public void subMillisecondFramesKeepMoonlightXsFractionalCarry() {
        PyroWaveSubmissionTiming timing = new PyroWaveSubmissionTiming();
        VideoStats window = new VideoStats();
        for (int i = 0; i < 120; i++) {
            window.decoderTimeMs += timing.addDurationNs(760_000);
            window.totalFramesReceived++;
        }
        assertEquals(91, window.decoderTimeMs);
        assertEquals(91f / 120, window.getWindowDecoderLatencyMs(true), 0);
        // The remaining 0.2 ms was not discarded when the displayed value was rounded.
        assertEquals(1, timing.addDurationNs(800_000));
    }

    @Test
    public void carryCrossesWindowFlipsAndOnlyLastTwoWindowsAreDisplayed() {
        PyroWaveSubmissionTiming timing = new PyroWaveSubmissionTiming();
        VideoStats active = new VideoStats();
        VideoStats previous = new VideoStats();
        VideoStats global = new VideoStats();
        for (int window = 0; window < 5; window++) {
            for (int frame = 0; frame < 3; frame++) {
                active.decoderTimeMs += timing.addDurationNs(400_000);
                active.totalFramesReceived++;
            }
            VideoStats lastTwo = new VideoStats();
            lastTwo.add(previous);
            lastTwo.add(active);
            assertEquals(window == 4 ? 0.5f : 1f / 3, lastTwo.getWindowDecoderLatencyMs(true), 0);
            global.add(active);
            previous.copy(active);
            active.clear();
        }
        assertEquals(6, global.decoderTimeMs);
        assertEquals(15, global.totalFramesReceived);
    }

    @Test
    public void skippedAndInvalidFramesStayInTheReceivedDenominator() {
        PyroWaveSubmissionTiming timing = new PyroWaveSubmissionTiming();
        VideoStats window = new VideoStats();
        // Two accepted submissions and one native-rejected submission all cost CPU time.
        for (long duration : new long[] {800_000, 800_000, 400_000}) {
            window.decoderTimeMs += timing.addDurationNs(duration);
            window.totalFramesReceived++;
        }
        // The fourth frame was superseded before entering native code.
        window.totalFramesReceived++;
        assertEquals(0.5f, window.getWindowDecoderLatencyMs(true), 0);
    }

    @Test
    public void submissionAverageDoesNotReplaceCompletedDecodeSamples() {
        PyroWaveSubmissionTiming timing = new PyroWaveSubmissionTiming();
        VideoStats window = new VideoStats();
        FrameLatencyStats completion = new FrameLatencyStats();
        for (int frame = 1; frame <= 2; frame++) {
            long inputNs = frame * 100_000_000L;
            completion.onDecoderInput(frame, frame, inputNs, inputNs, inputNs, (char) 0);
            completion.onDecoderQueued(frame, inputNs);
            window.decoderTimeMs += timing.addDurationNs(700_000);
            window.totalFramesReceived++;
            // The asynchronous GPU completion arrives much later than native submission.
            completion.onDecoderOutput(frame, frame, inputNs + frame * 4_000_000L);
        }
        assertEquals(0.5f, window.getWindowDecoderLatencyMs(true), 0);
        assertEquals(6, completion.takeDecodeTimeMs(), 0);
        assertEquals(6, completion.summarize()[1][1], 0);
        assertEquals(8, completion.summarize()[1][2], 0);
        assertEquals(0, completion.getArtemisDecodeMs());
    }
}

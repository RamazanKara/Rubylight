package com.limelight.binding.video;

import android.app.Application;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import static org.junit.Assert.assertEquals;

/**
 * Regression (nightjar#80): frames the engine dropped over a full decode queue reached Java as a
 * frame-number gap and counted as network loss, which the overlay, the stream doctor and the bitrate
 * logic then acted on. They are decoder drops now, on their own overlay line.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class)
public class DecoderDropsTest {
    @Test
    public void aGapAcrossADecodeQueueOverflowIsDecoderDropsNotLoss() {
        VideoStats window = new VideoStats();
        MediaCodecDecoderRenderer.recordFrameGap(window, 21, true);
        assertEquals(0, window.framesLost);
        assertEquals(0, window.frameLossEvents);
        assertEquals(21, window.decoderDrops);
        assertEquals(21, window.totalFrames);
        window.totalFramesReceived = 79;
        window.totalFrames += 79;
        VideoStatsFps fps = window.getFps();
        assertEquals(0, fps.lossPercent, 0);
        assertEquals(21, fps.decoderDrops);
    }

    @Test
    public void aGapWithoutAnOverflowIsStillNetworkLoss() {
        VideoStats window = new VideoStats();
        MediaCodecDecoderRenderer.recordFrameGap(window, 2, false);
        window.totalFrames += 98;
        assertEquals(2, window.framesLost);
        assertEquals(1, window.frameLossEvents);
        assertEquals(0, window.decoderDrops);
        assertEquals(2, window.getFps().lossPercent, 0.0001f);
    }

    @Test
    public void decoderDropsSurviveTheWindowRollover() {
        VideoStats window = new VideoStats();
        MediaCodecDecoderRenderer.recordFrameGap(window, 5, true);
        VideoStats last = new VideoStats();
        last.copy(window);
        VideoStats both = new VideoStats();
        both.add(last);
        both.add(window);
        assertEquals(10, both.decoderDrops);
        window.clear();
        assertEquals(0, window.decoderDrops);
    }
}

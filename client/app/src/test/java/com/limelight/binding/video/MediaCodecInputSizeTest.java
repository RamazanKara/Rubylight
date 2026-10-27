package com.limelight.binding.video;

import android.app.Application;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import static org.junit.Assert.assertEquals;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class MediaCodecInputSizeTest {
    @Test
    public void ordinaryBitratesKeepTheDecodersOwnSize() {
        assertEquals(0, MediaCodecDecoderRenderer.maxInputSize(80000, 60));
        assertEquals(0, MediaCodecDecoderRenderer.maxInputSize(250000, 120));
        assertEquals(0, MediaCodecDecoderRenderer.maxInputSize(0, 60));
        assertEquals(0, MediaCodecDecoderRenderer.maxInputSize(500000, 0));
    }

    @Test
    public void highBitratesFitFourAverageFrames() {
        // 1 Gbps at 60 FPS averages 2083333 bytes a frame.
        assertEquals(8333332, MediaCodecDecoderRenderer.maxInputSize(1000000, 60));
    }

    @Test
    public void inputSizeIsCapped() {
        assertEquals(32 * 1024 * 1024, MediaCodecDecoderRenderer.maxInputSize(2000000, 24));
    }
}

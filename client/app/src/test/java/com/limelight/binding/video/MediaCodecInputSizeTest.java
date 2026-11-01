package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.media.MediaFormat;

import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class MediaCodecInputSizeTest {
    @Test
    public void ordinaryBitratesKeepTheDecodersOwnSize() {
        assertEquals(0, MediaCodecDecoderRenderer.maxInputSize(80000, 60, 1920, 1080));
        assertEquals(0, MediaCodecDecoderRenderer.maxInputSize(250000, 120, 1920, 1080));
        assertEquals(0, MediaCodecDecoderRenderer.maxInputSize(0, 60, 3840, 2160));
        assertEquals(0, MediaCodecDecoderRenderer.maxInputSize(500000, 0, 3840, 2160));
    }

    @Test
    public void highBitratesFitFourAverageFrames() {
        // 1 Gbps at 60 FPS averages 2083333 bytes a frame.
        assertEquals(8333332, MediaCodecDecoderRenderer.maxInputSize(1000000, 60, 1920, 1080));
    }

    @Test
    public void largeStreamsFitAKeyframeOfAByteAPixel() {
        // Issue 56: a 4K keyframe at 160 Mbps was 4.4 MB and the MediaTek decoder gave 4 MB, so
        // the buffer grew by restarting the decoder mid-stream.
        assertEquals(3840 * 2160, MediaCodecDecoderRenderer.maxInputSize(158988, 60, 3840, 2160));
        assertTrue(MediaCodecDecoderRenderer.maxInputSize(158988, 60, 3840, 2160) > 4462353);
        assertEquals(2560 * 1440, MediaCodecDecoderRenderer.maxInputSize(80000, 60, 2560, 1440));
    }

    @Test
    public void inputSizeIsCapped() {
        assertEquals(32 * 1024 * 1024, MediaCodecDecoderRenderer.maxInputSize(2000000, 24, 3840, 2160));
    }

    @Test
    public void aFrameThatDoesNotFitDoublesTheBufferOrTakesItsOwnSize() {
        int mib = 1024 * 1024;
        assertEquals(4 * mib, MediaCodecDecoderRenderer.grownInputSize(3L * mib, 2 * mib, 0));
        assertEquals(5 * mib, MediaCodecDecoderRenderer.grownInputSize(5L * mib, 2 * mib, 0));
        assertEquals(0, MediaCodecDecoderRenderer.grownInputSize(2L * mib, 2 * mib, 0));
        // The decoder ignored a request this large already, so asking again cannot help.
        assertEquals(0, MediaCodecDecoderRenderer.grownInputSize(3L * mib, 2 * mib, 4 * mib));
        assertEquals(6 * mib, MediaCodecDecoderRenderer.grownInputSize(5L * mib, 3 * mib, 4 * mib));
        assertEquals(0, MediaCodecDecoderRenderer.grownInputSize(Integer.MAX_VALUE + 1L, 2 * mib, 0));
        assertEquals(Integer.MAX_VALUE, MediaCodecDecoderRenderer.grownInputSize(Integer.MAX_VALUE, Integer.MAX_VALUE - 1, 0));
    }

    @Test
    public void everyConfigureKeepsTheGrownSize() {
        MediaFormat plain = MediaFormat.createVideoFormat("video/hevc", 1920, 1080);
        MediaCodecDecoderRenderer.applyRequiredInputSize(plain, 0);
        assertFalse(plain.containsKey(MediaFormat.KEY_MAX_INPUT_SIZE));
        MediaCodecDecoderRenderer.applyRequiredInputSize(plain, 4000000);
        assertEquals(4000000, plain.getInteger(MediaFormat.KEY_MAX_INPUT_SIZE));
        // A larger size from the bitrate stays.
        MediaFormat large = MediaFormat.createVideoFormat("video/hevc", 1920, 1080);
        large.setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, 8000000);
        MediaCodecDecoderRenderer.applyRequiredInputSize(large, 4000000);
        assertEquals(8000000, large.getInteger(MediaFormat.KEY_MAX_INPUT_SIZE));
    }

    @Test
    public void growthRestartsTheDecoderOnceAndThenGivesUp() {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity.get(),
                    new PreferenceConfiguration(), null, 0, false, false, "", null);
            AtomicInteger recovery = ReflectionHelpers.getField(renderer, "codecRecoveryType");
            int none = ReflectionHelpers.getStaticField(MediaCodecDecoderRenderer.class, "CR_RECOVERY_TYPE_NONE");
            int restart = ReflectionHelpers.getStaticField(MediaCodecDecoderRenderer.class, "CR_RECOVERY_TYPE_RESTART");
            assertEquals(none, recovery.get());

            assertTrue(renderer.requestInputBufferGrowth(3000000, 2000000));
            assertEquals(4000000, (int) ReflectionHelpers.getField(renderer, "requiredInputBufferSize"));
            assertEquals(restart, recovery.get());

            // The restarted decoder still gave a 2 MB buffer: the same frame now ends the stream.
            recovery.set(none);
            assertFalse(renderer.requestInputBufferGrowth(3000000, 2000000));
            assertEquals(none, recovery.get());

            // A bigger frame grows again, unless the recovery attempts are used up.
            ReflectionHelpers.setField(renderer, "codecRecoveryAttempts", 10);
            assertFalse(renderer.requestInputBufferGrowth(6000000, 4000000));
            ReflectionHelpers.setField(renderer, "codecRecoveryAttempts", 9);
            assertTrue(renderer.requestInputBufferGrowth(6000000, 4000000));
            assertEquals(8000000, (int) ReflectionHelpers.getField(renderer, "requiredInputBufferSize"));
        }
    }
}

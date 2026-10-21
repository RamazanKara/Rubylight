package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.media.MediaCodec;
import android.media.MediaFormat;

import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.shadows.ShadowMediaCodec;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, shadows = MediaCodecOutputFormatTest.FormatChangingCodec.class)
public class MediaCodecOutputFormatTest {
    @Test
    public void packedChromaSeparatesDisplaySizeFromCodedPadding() {
        Packed444 geometry = new Packed444(2559, 1439, 2, 0);
        assertEquals(2559, geometry.width);
        assertEquals(1439, geometry.height);
        assertEquals(2560, geometry.paddedWidth);
        assertEquals(2880, geometry.paddedHeight * 2);
        assertEquals(2, geometry.colorSpace);
        assertEquals(0, geometry.colorRange);
    }

    @Test
    @Config(sdk = {23, 28, 29})
    public void packedSamplesBypassDecoderColorConversion() {
        MediaFormat format = MediaFormat.createVideoFormat("video/hevc", 2560, 2880);
        format.setInteger(MediaFormat.KEY_COLOR_STANDARD, MediaFormat.COLOR_STANDARD_BT2020);
        format.setInteger(MediaFormat.KEY_COLOR_TRANSFER, MediaFormat.COLOR_TRANSFER_ST2084);
        format.setInteger(MediaFormat.KEY_COLOR_RANGE, MediaFormat.COLOR_RANGE_LIMITED);
        format.setByteBuffer(MediaFormat.KEY_HDR_STATIC_INFO, java.nio.ByteBuffer.allocate(25));
        Packed444.sampleDataFormat(format);
        assertEquals(MediaFormat.COLOR_STANDARD_BT709, format.getInteger(MediaFormat.KEY_COLOR_STANDARD));
        assertEquals(MediaFormat.COLOR_TRANSFER_SDR_VIDEO, format.getInteger(MediaFormat.KEY_COLOR_TRANSFER));
        assertEquals(MediaFormat.COLOR_RANGE_FULL, format.getInteger(MediaFormat.KEY_COLOR_RANGE));
        assertNull(format.getByteBuffer(MediaFormat.KEY_HDR_STATIC_INFO));
    }

    @Test
    public void packedChromaRequiresOptInAndTheRustEngine() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
            MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity.get(), prefs,
                    null, 0, false, false, "", null);
            assertEquals(-1, renderer.setChromaPacking(1, 2560, 1440, 1, 0));
            prefs.experimentalPacked444 = true;
            assertEquals(-1, renderer.setChromaPacking(1, 2560, 1440, 1, 0));
            prefs.rustEngine = true;
            assertEquals(-1, renderer.setChromaPacking(2, 2560, 1440, 1, 0));
            assertEquals(0, renderer.setChromaPacking(1, 2560, 1440, 1, 0));
            assertEquals(0, renderer.setChromaPacking(0, 2560, 1440, 1, 0));
            assertNull(ReflectionHelpers.getField(renderer, "packing"));
        }
    }

    @Test
    public void drainingQueuedFramesStillProcessesFormatChanges() throws Exception {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
            prefs.framePacing = PreferenceConfiguration.FRAME_PACING_MIN_LATENCY;
            MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity.get(), prefs,
                    null, 0, false, false, "", null);
            renderer.setStreamFrameRateMilliHz(60_000);
            FormatChangingCodec.renderer = renderer;
            FormatChangingCodec.format = MediaFormat.createVideoFormat("video/avc", 1920, 1080);
            MediaCodec codec = MediaCodec.createDecoderByType("video/avc");
            try {
                ReflectionHelpers.setField(renderer, "videoDecoder", codec);
                ReflectionHelpers.callInstanceMethod(renderer, "startRendererThread");
                Thread thread = ReflectionHelpers.getField(renderer, "rendererThread");
                try {
                    thread.join(5000);
                    assertFalse("Renderer did not release the queued frame", thread.isAlive());
                    assertSame(FormatChangingCodec.format, ReflectionHelpers.getField(renderer, "outputFormat"));
                } finally {
                    renderer.stop();
                }
            } finally {
                codec.release();
            }
        }
    }

    @Implements(MediaCodec.class)
    public static class FormatChangingCodec extends ShadowMediaCodec {
        static MediaCodecDecoderRenderer renderer;
        static MediaFormat format;
        private int dequeues;

        @Implementation
        public int dequeueOutputBuffer(MediaCodec.BufferInfo info, long timeoutUs) {
            info.presentationTimeUs = 1;
            return dequeues++ == 0 ? 0 : MediaCodec.INFO_OUTPUT_FORMAT_CHANGED;
        }

        @Implementation
        public MediaFormat getOutputFormat() {
            return format;
        }

        @Implementation
        public void releaseOutputBuffer(int index, long timestampNs) {
            ReflectionHelpers.setField(renderer, "stopping", true);
        }
    }
}

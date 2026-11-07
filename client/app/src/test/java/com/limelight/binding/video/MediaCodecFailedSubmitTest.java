package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.graphics.SurfaceTexture;
import android.media.MediaCodec;
import android.media.MediaCodecInfo;
import android.media.MediaCrypto;
import android.media.MediaFormat;
import android.view.Surface;

import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.shadows.MediaCodecInfoBuilder;
import org.robolectric.shadows.ShadowMediaCodec;
import org.robolectric.util.ReflectionHelpers;
import org.robolectric.util.ReflectionHelpers.ClassParameter;

import java.nio.ByteBuffer;

import static org.junit.Assert.assertEquals;

/**
 * A frame the codec refuses is not in the decoder: the frames-in count, and with it the depth that
 * Smoothest, the frame-rate debt and the residency governor wait on, only counts queued frames.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class,
        shadows = {MediaCodecFailedSubmitTest.RefusingCodec.class, com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class MediaCodecFailedSubmitTest {
    /** Takes buffers, but refuses the next one with a transient error when told to. */
    @Implements(MediaCodec.class)
    public static class RefusingCodec extends ShadowMediaCodec {
        static int queued;
        static boolean refuseNext;

        @Implementation public void configure(MediaFormat format, Surface surface, MediaCrypto crypto, int flags) {}
        @Implementation public void start() {}
        @Implementation public int dequeueInputBuffer(long timeoutUs) {
            return 0;
        }
        @Implementation public ByteBuffer getInputBuffer(int index) {
            return ByteBuffer.allocate(4096);
        }
        @Implementation public MediaCodecInfo getCodecInfo() {
            return null;
        }
        @Implementation public void queueInputBuffer(int index, int offset, int size, long timeUs, int flags) {
            if (refuseNext) {
                refuseNext = false;
                // ACTION_TRANSIENT: the renderer keeps the buffer and asks for an IDR, no recovery.
                throw ReflectionHelpers.callConstructor(MediaCodec.CodecException.class,
                        ClassParameter.from(int.class, 0), ClassParameter.from(int.class, 2),
                        ClassParameter.from(String.class, "busy"));
            }
            queued++;
        }
    }

    private ActivityController<Activity> activity;
    private SurfaceTexture texture;
    private Surface surface;
    private MediaCodecDecoderRenderer renderer;
    private int frame;

    @Before
    public void setUp() {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        RefusingCodec.queued = 0;
        RefusingCodec.refuseNext = false;
        activity = Robolectric.buildActivity(Activity.class).create();
        texture = new SurfaceTexture(0);
        surface = new Surface(texture);
        frame = 0;

        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.FORCE_HEVC;
        renderer = new MediaCodecDecoderRenderer(activity.get(), prefs, null, 0, false, false, "", null);
        ReflectionHelpers.setField(renderer, "streamFrameRateMilliHz", 60_000);
        ReflectionHelpers.setField(renderer, "renderTarget", surface);
        ReflectionHelpers.setField(renderer, "videoFormat", MoonBridge.VIDEO_FORMAT_H265);
        MediaCodecInfo info = MediaCodecInfoBuilder.newBuilder().setName("c2.qti.hevc.decoder").build();
        ReflectionHelpers.callInstanceMethod(renderer, "tryConfigureDecoder",
                ClassParameter.from(MediaCodecInfo.class, info),
                ClassParameter.from(MediaFormat.class, MediaFormat.createVideoFormat("video/hevc", 1920, 1080)),
                ClassParameter.from(boolean.class, true));
    }

    @After
    public void tearDown() {
        surface.release();
        texture.release();
        activity.close();
    }

    private int submit(int frameType) {
        frame++;
        return renderer.submitDecodeUnit(new byte[64], 64, MoonBridge.BUFFER_TYPE_PICDATA, frame, frameType, (char) 0,
                frame * 16_000L, frame * 16_000L, frame * 16_000_000L, frame * 16_000L);
    }

    private int depth() {
        int in = ReflectionHelpers.getField(renderer, "numFramesIn");
        int out = ReflectionHelpers.getField(renderer, "numFramesOut");
        int base = ReflectionHelpers.getField(renderer, "decoderDepthBase");
        return DecoderQueuePolicy.depth(in, out, base);
    }

    @Test
    public void aRefusedFrameIsNotCountedAsInTheDecoder() {
        assertEquals(MoonBridge.DR_OK, submit(MoonBridge.FRAME_TYPE_IDR));
        assertEquals(1, RefusingCodec.queued);
        assertEquals(1, depth());

        RefusingCodec.refuseNext = true;
        assertEquals(MoonBridge.DR_NEED_IDR, submit(MoonBridge.FRAME_TYPE_PFRAME));
        assertEquals(1, RefusingCodec.queued);
        // Before the fix the refused frame stayed counted until the next recovery.
        assertEquals("a refused frame is not in the decoder", 1, depth());
        assertEquals(1, (int) ReflectionHelpers.getField(renderer, "numFramesIn"));

        assertEquals(MoonBridge.DR_OK, submit(MoonBridge.FRAME_TYPE_IDR));
        assertEquals(2, RefusingCodec.queued);
        assertEquals(2, depth());
    }
}

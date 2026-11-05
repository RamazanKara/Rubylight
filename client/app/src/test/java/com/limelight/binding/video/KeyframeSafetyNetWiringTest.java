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
import org.robolectric.shadows.ShadowBuild;
import org.robolectric.shadows.ShadowMediaCodec;
import org.robolectric.shadows.ShadowSystemClock;
import org.robolectric.util.ReflectionHelpers;
import org.robolectric.util.ReflectionHelpers.ClassParameter;

import java.nio.ByteBuffer;
import java.time.Duration;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;

/**
 * The stream's side of {@link KeyframeSafetyNet}: only the one Fire TV's MediaTek decoder in Android's
 * low-latency mode asks for keyframes, the frame that asks still decodes, and any keyframe restarts it.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class,
        shadows = {KeyframeSafetyNetWiringTest.QueueingCodec.class, com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class KeyframeSafetyNetWiringTest {
    private static final String MODEL = "AFTNE4E43F";
    private static final String MTK = "c2.mtk.hevc.decoder";

    /** Takes every buffer it is given, as a decoder with room does. */
    @Implements(MediaCodec.class)
    public static class QueueingCodec extends ShadowMediaCodec {
        static int queued;

        @Implementation public void configure(MediaFormat format, Surface surface, MediaCrypto crypto, int flags) {}
        @Implementation public void start() {}
        @Implementation public int dequeueInputBuffer(long timeoutUs) {
            return 0;
        }
        @Implementation public ByteBuffer getInputBuffer(int index) {
            return ByteBuffer.allocate(4096);
        }
        @Implementation public void queueInputBuffer(int index, int offset, int size, long timeUs, int flags) {
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
        QueueingCodec.queued = 0;
        activity = Robolectric.buildActivity(Activity.class).create();
        texture = new SurfaceTexture(0);
        surface = new Surface(texture);
        frame = 0;
    }

    @After
    public void tearDown() {
        surface.release();
        texture.release();
        activity.close();
    }

    private void configure(String model, String decoder, boolean lowLatency, boolean hevcOverrideOff) {
        ShadowBuild.setModel(model);
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.FORCE_HEVC;
        prefs.codecLowLatency = lowLatency;
        prefs.disableHevcLowLatency = hevcOverrideOff;
        renderer = new MediaCodecDecoderRenderer(activity.get(), prefs, null, 0, false, false, "", null);
        ReflectionHelpers.setField(renderer, "renderTarget", surface);
        ReflectionHelpers.setField(renderer, "videoFormat", MoonBridge.VIDEO_FORMAT_H265);
        ReflectionHelpers.callInstanceMethod(renderer, "tryConfigureDecoder",
                ClassParameter.from(MediaCodecInfo.class, MediaCodecInfoBuilder.newBuilder().setName(decoder).build()),
                ClassParameter.from(MediaFormat.class, MediaFormat.createVideoFormat("video/hevc", 3840, 2160)),
                ClassParameter.from(boolean.class, true));
    }

    private int submit(int frameType) {
        frame++;
        return renderer.submitDecodeUnit(new byte[64], 64, MoonBridge.BUFFER_TYPE_PICDATA, frame, frameType, (char) 0,
                frame * 16_000L, frame * 16_000L, frame * 16_000_000L, frame * 16_000L);
    }

    private static void advance(long ms) {
        ShadowSystemClock.advanceBy(Duration.ofMillis(ms));
    }

    @Test
    public void itAsksEveryTenSecondsAndTheAskingFrameStillDecodes() {
        configure(MODEL, MTK, true, false);
        assertNotNull(ReflectionHelpers.getField(renderer, "keyframeSafetyNet"));
        assertEquals(MoonBridge.DR_OK, submit(MoonBridge.FRAME_TYPE_IDR));
        assertEquals(MoonBridge.DR_OK, submit(MoonBridge.FRAME_TYPE_PFRAME));
        advance(9_999);
        assertEquals(MoonBridge.DR_OK, submit(MoonBridge.FRAME_TYPE_PFRAME));
        advance(1);
        int queued = QueueingCodec.queued;
        assertEquals(MoonBridge.DR_NEED_IDR, submit(MoonBridge.FRAME_TYPE_PFRAME));
        assertEquals("the frame that asks is queued", queued + 1, QueueingCodec.queued);
        // Frames in flight until the keyframe do not ask again.
        assertEquals(MoonBridge.DR_OK, submit(MoonBridge.FRAME_TYPE_PFRAME));
    }

    @Test
    public void anyKeyframeRestartsTheWaitAndNeverAsksItself() {
        configure(MODEL, MTK, true, false);
        assertEquals(MoonBridge.DR_OK, submit(MoonBridge.FRAME_TYPE_IDR));
        advance(9_000);
        assertEquals("an IDR after loss counts", MoonBridge.DR_OK, submit(MoonBridge.FRAME_TYPE_IDR));
        advance(9_999);
        assertEquals(MoonBridge.DR_OK, submit(MoonBridge.FRAME_TYPE_PFRAME));
        advance(30_000);
        assertEquals("a keyframe past the time answers it", MoonBridge.DR_OK, submit(MoonBridge.FRAME_TYPE_IDR));
        advance(100);
        assertEquals(MoonBridge.DR_OK, submit(MoonBridge.FRAME_TYPE_PFRAME));
    }

    @Test
    public void lowLatencyOffNeverAsks() {
        configure(MODEL, MTK, false, false);
        assertUntouched();
    }

    @Test
    public void theHevcOverrideLeavingTheFlagOutNeverAsks() {
        configure(MODEL, MTK, true, true);
        assertUntouched();
    }

    @Test
    public void anotherModelWithTheSameDecoderNeverAsks() {
        configure("AFTMM", MTK, true, false);
        assertUntouched();
    }

    @Test
    public void theModelWithAnotherDecoderNeverAsks() {
        configure(MODEL, "c2.amlogic.hevc.decoder", true, false);
        assertUntouched();
    }

    private void assertUntouched() {
        assertNull(ReflectionHelpers.getField(renderer, "keyframeSafetyNet"));
        assertEquals(MoonBridge.DR_OK, submit(MoonBridge.FRAME_TYPE_IDR));
        for (int i = 0; i < 3; i++) {
            advance(20_000);
            assertEquals(MoonBridge.DR_OK, submit(MoonBridge.FRAME_TYPE_PFRAME));
        }
    }
}

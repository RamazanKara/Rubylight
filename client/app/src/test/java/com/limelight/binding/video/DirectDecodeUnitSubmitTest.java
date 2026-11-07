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
import org.robolectric.util.ReflectionHelpers;
import org.robolectric.util.ReflectionHelpers.ClassParameter;

import java.nio.ByteBuffer;
import java.util.Arrays;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;

/**
 * Picture data handed over in place (submitDirectDecodeUnit) when Java reads the frame's bytes: Dolby
 * Vision's trim, the HDR Vivid SEI drop, the Vulkan renderer's HDR10+ parse. Whether the switch was on
 * before the hand-off or turned on between the check and submitDecodeUnit (the race that left a null
 * array), the codec gets the right bytes and nothing throws.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class,
        shadows = {DirectDecodeUnitSubmitTest.CapturingCodec.class, com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class DirectDecodeUnitSubmitTest {
    /** Takes every buffer and keeps the bytes of the last one queued. */
    @Implements(MediaCodec.class)
    public static class CapturingCodec extends ShadowMediaCodec {
        static ByteBuffer lastInput;
        static byte[] queuedBytes;

        @Implementation public void configure(MediaFormat format, Surface surface, MediaCrypto crypto, int flags) {}
        @Implementation public void start() {}
        @Implementation public int dequeueInputBuffer(long timeoutUs) {
            return 0;
        }
        @Implementation public ByteBuffer getInputBuffer(int index) {
            lastInput = ByteBuffer.allocate(4096);
            return lastInput;
        }
        @Implementation public MediaCodecInfo getCodecInfo() {
            return null;
        }
        @Implementation public void queueInputBuffer(int index, int offset, int size, long timeUs, int flags) {
            queuedBytes = Arrays.copyOfRange(lastInput.array(), offset, offset + size);
        }
    }

    // An HEVC picture whose FEC padding (the zeros) Dolby Vision's trim drops.
    private static final byte[] PICTURE = {0, 0, 0, 1, 0x02, 0x01, (byte) 0xd0, 0x11, 0x22, 0x33, 0x44, 0, 0, 0};
    private static final int TRIMMED = 11;

    private ActivityController<Activity> activity;
    private SurfaceTexture texture;
    private Surface surface;
    private MediaCodecDecoderRenderer renderer;
    private int frame;

    @Before
    public void setUp() {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        CapturingCodec.lastInput = null;
        CapturingCodec.queuedBytes = null;
        activity = Robolectric.buildActivity(Activity.class).create();
        texture = new SurfaceTexture(0);
        surface = new Surface(texture);
        frame = 0;

        ShadowBuild.setModel("Pixel 8");
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.FORCE_HEVC;
        prefs.bitrate = 20_000;
        renderer = new MediaCodecDecoderRenderer(activity.get(), prefs, null, 0, false, false, "", null);
        ReflectionHelpers.setField(renderer, "streamFrameRateMilliHz", 60_000);
        ReflectionHelpers.setField(renderer, "renderTarget", surface);
        ReflectionHelpers.setField(renderer, "videoFormat", MoonBridge.VIDEO_FORMAT_H265);
        MediaFormat format = MediaFormat.createVideoFormat("video/hevc", 1920, 1080);
        MediaCodecInfo info = MediaCodecInfoBuilder.newBuilder().setName("c2.android.hevc.decoder").build();
        ReflectionHelpers.callInstanceMethod(renderer, "tryConfigureDecoder",
                ClassParameter.from(MediaCodecInfo.class, info),
                ClassParameter.from(MediaFormat.class, format),
                ClassParameter.from(boolean.class, true));
        ReflectionHelpers.setField(renderer, "inputSpacer", null);
    }

    @After
    public void tearDown() {
        surface.release();
        texture.release();
        activity.close();
    }

    private static ByteBuffer direct() {
        ByteBuffer buffer = ByteBuffer.allocateDirect(PICTURE.length);
        buffer.put(PICTURE);
        return buffer;
    }

    private int submitDirect() {
        frame++;
        return renderer.submitDirectDecodeUnit(direct(), PICTURE.length, MoonBridge.BUFFER_TYPE_PICDATA, frame,
                frame == 1 ? MoonBridge.FRAME_TYPE_IDR : MoonBridge.FRAME_TYPE_PFRAME, (char) 0,
                frame * 16_000L, frame * 16_000L, frame * 16_000_000L, frame * 16_000L);
    }

    // The state submitDirectDecodeUnit leaves when a switch turned on after its check: the frame in place
    // and no array.
    private int submitAfterTheCheck() {
        frame++;
        ReflectionHelpers.setField(renderer, "directInput", direct());
        try {
            return renderer.submitDecodeUnit(null, PICTURE.length, MoonBridge.BUFFER_TYPE_PICDATA, frame,
                    frame == 1 ? MoonBridge.FRAME_TYPE_IDR : MoonBridge.FRAME_TYPE_PFRAME, (char) 0,
                    frame * 16_000L, frame * 16_000L, frame * 16_000_000L, frame * 16_000L);
        } finally {
            ReflectionHelpers.setField(renderer, "directInput", null);
        }
    }

    private void assertQueued(int length) {
        assertArrayEquals(Arrays.copyOf(PICTURE, length), CapturingCodec.queuedBytes);
    }

    @Test
    public void withNothingReadingTheBytesTheWholeFrameGoesInPlace() {
        assertEquals(MoonBridge.DR_OK, submitDirect());
        assertQueued(PICTURE.length);
        assertNull(ReflectionHelpers.getField(renderer, "directArray"));
    }

    @Test
    public void dolbyVisionTrimsADirectFrame() {
        ReflectionHelpers.setField(renderer, "dolbyVisionActive", true);
        assertEquals(MoonBridge.DR_OK, submitDirect());
        assertQueued(TRIMMED);
    }

    @Test
    public void dolbyVisionTurningOnAfterTheCheckTrimsWithoutANullArray() {
        assertEquals(MoonBridge.DR_OK, submitDirect());
        ReflectionHelpers.setField(renderer, "dolbyVisionActive", true);
        assertEquals(MoonBridge.DR_OK, submitAfterTheCheck());
        assertQueued(TRIMMED);
    }

    @Test
    public void theVividDropReadsADirectFrame() {
        ReflectionHelpers.setField(renderer, "vividSeiDropDecoder", "c2.mtk.hevc.decoder");
        assertEquals(MoonBridge.DR_OK, submitDirect());
        assertQueued(PICTURE.length);
        assertEquals(MoonBridge.DR_OK, submitAfterTheCheck());
        assertQueued(PICTURE.length);
    }

    @Test
    public void theVulkanHdr10PlusParseReadsADirectFrame() {
        ReflectionHelpers.setField(renderer, "vulkanHdr10Plus", true);
        ReflectionHelpers.setField(renderer, "currentHdrMode", Boolean.TRUE);
        assertEquals(MoonBridge.DR_OK, submitDirect());
        assertQueued(PICTURE.length);
        assertEquals(MoonBridge.DR_OK, submitAfterTheCheck());
        assertQueued(PICTURE.length);
    }
}

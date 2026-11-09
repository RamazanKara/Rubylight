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

    private static final byte[] AUD = Hdr10PlusSeiTest.bytes(Hdr10PlusSeiTest.HEVC_AUD);
    private static final byte[] HDR10_PLUS = Hdr10PlusSeiTest.bytes(Hdr10PlusSeiTest.HEVC_SEI);
    private static final byte[] VIVID = Hdr10PlusSeiTest.bytes(Hdr10PlusSeiTest.HEVC_VIVID);
    private static final byte[] SLICE = Hdr10PlusSeiTest.bytes(Hdr10PlusSeiTest.HEVC_SLICE);
    private static final byte[] PADDING = new byte[37];
    // An HEVC picture as the host sends HDR: both dynamic metadata SEIs, the slice, then FEC padding
    // (the zeros) that Dolby Vision's trim drops.
    private static final byte[] PICTURE = concat(AUD, HDR10_PLUS, VIVID, SLICE, PADDING);
    private static final int TRIMMED = PICTURE.length - PADDING.length;

    private static byte[] concat(byte[]... parts) {
        java.io.ByteArrayOutputStream out = new java.io.ByteArrayOutputStream();
        for (byte[] part : parts) out.write(part, 0, part.length);
        return out.toByteArray();
    }

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
        return submitDirect(direct());
    }

    private int submitDirect(ByteBuffer direct) {
        frame++;
        return renderer.submitDirectDecodeUnit(direct, PICTURE.length, MoonBridge.BUFFER_TYPE_PICDATA, frame,
                frame == 1 ? MoonBridge.FRAME_TYPE_IDR : MoonBridge.FRAME_TYPE_PFRAME, (char) 0,
                frame * 16_000L, frame * 16_000L, frame * 16_000_000L, frame * 16_000L);
    }

    private int submitArray() {
        frame++;
        // In a larger reused buffer, as the bridge passes it: only the length counts.
        byte[] array = Arrays.copyOf(PICTURE, PICTURE.length + 16);
        Arrays.fill(array, PICTURE.length, array.length, (byte) 0x55);
        return renderer.submitDecodeUnit(array, PICTURE.length, MoonBridge.BUFFER_TYPE_PICDATA, frame,
                frame == 1 ? MoonBridge.FRAME_TYPE_IDR : MoonBridge.FRAME_TYPE_PFRAME, (char) 0,
                frame * 16_000L, frame * 16_000L, frame * 16_000_000L, frame * 16_000L);
    }

    // The base class's array copy of a frame handed over in place: never made for picture data.
    private Object directCopy() {
        return ReflectionHelpers.getField(renderer, "directCopy");
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
        assertNull(directCopy());
    }

    @Test
    public void dolbyVisionTrimsADirectFrame() {
        ReflectionHelpers.setField(renderer, "dolbyVisionActive", true);
        assertEquals(MoonBridge.DR_OK, submitDirect());
        assertQueued(TRIMMED);
        assertNull(directCopy());
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
        byte[] withoutVivid = concat(AUD, HDR10_PLUS, SLICE, PADDING);
        assertEquals(MoonBridge.DR_OK, submitDirect());
        assertArrayEquals(withoutVivid, CapturingCodec.queuedBytes);
        assertEquals(MoonBridge.DR_OK, submitAfterTheCheck());
        assertArrayEquals(withoutVivid, CapturingCodec.queuedBytes);
        assertNull(directCopy());
    }

    @Test
    public void theVulkanHdr10PlusParseReadsADirectFrame() {
        useVulkan();
        ReflectionHelpers.setField(renderer, "vulkanHdr10Plus", true);
        assertEquals(MoonBridge.DR_OK, submitDirect());
        assertQueued(PICTURE.length);
        assertArrayEquals(Hdr10PlusSeiTest.bytes(Hdr10PlusSeiTest.T35),
                (byte[]) ReflectionHelpers.getField(renderer, "vulkanHdr10PlusNoted"));
        assertEquals(MoonBridge.DR_OK, submitAfterTheCheck());
        assertQueued(PICTURE.length);
        assertNull(directCopy());
    }

    // A renderer without a native one behind it: what the frame notes reach is dropped.
    private void useVulkan() {
        ReflectionHelpers.setField(renderer, "vulkanRenderer", ReflectionHelpers.callConstructor(
                VulkanRendererBridge.class, ClassParameter.from(long.class, 0L)));
        ReflectionHelpers.setField(renderer, "currentHdrMode", Boolean.TRUE);
    }

    @Test
    public void everyCombinationQueuesFromPlaceWhatTheArrayQueues() {
        useVulkan();
        for (int combination = 0; combination < 8; combination++) {
            boolean dolbyVision = (combination & 1) != 0;
            boolean vivid = (combination & 2) != 0;
            boolean hdr10Plus = (combination & 4) != 0;
            String name = "Dolby Vision " + dolbyVision + ", Vivid drop " + vivid + ", HDR10+ parse " + hdr10Plus;
            ReflectionHelpers.setField(renderer, "dolbyVisionActive", dolbyVision);
            ReflectionHelpers.setField(renderer, "vividSeiDropDecoder", vivid ? "c2.mtk.hevc.decoder" : null);
            ReflectionHelpers.setField(renderer, "vulkanHdr10Plus", hdr10Plus);
            byte[] expected = concat(AUD, HDR10_PLUS, vivid ? new byte[0] : VIVID, SLICE,
                    dolbyVision ? new byte[0] : PADDING);
            byte[] t35 = hdr10Plus ? Hdr10PlusSeiTest.bytes(Hdr10PlusSeiTest.T35) : null;

            ReflectionHelpers.setField(renderer, "vulkanHdr10PlusNoted", null);
            assertEquals(name, MoonBridge.DR_OK, submitArray());
            byte[] fromArray = CapturingCodec.queuedBytes;
            assertArrayEquals(name, expected, fromArray);
            assertArrayEquals(name, t35, (byte[]) ReflectionHelpers.getField(renderer, "vulkanHdr10PlusNoted"));

            ReflectionHelpers.setField(renderer, "vulkanHdr10PlusNoted", null);
            ByteBuffer direct = direct();
            assertEquals(name, MoonBridge.DR_OK, submitDirect(direct));
            assertArrayEquals(name, fromArray, CapturingCodec.queuedBytes);
            assertArrayEquals(name, t35, (byte[]) ReflectionHelpers.getField(renderer, "vulkanHdr10PlusNoted"));
            // Read where it is: no array copy, and the engine's buffer as it was handed over.
            assertNull(name, directCopy());
            assertEquals(name, PICTURE.length, direct.position());
            assertEquals(name, PICTURE.length, direct.limit());
        }
    }
}

package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.media.MediaCodecInfo;
import android.media.MediaCodecInfo.CodecCapabilities;
import android.media.MediaCodecInfo.CodecProfileLevel;
import android.media.MediaFormat;

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
import org.robolectric.shadows.MediaCodecInfoBuilder;
import org.robolectric.shadows.MediaCodecInfoBuilder.CodecCapabilitiesBuilder;
import org.robolectric.shadows.ShadowMediaCodecList;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.*;

/**
 * Slices per frame and direct submit come from the decoder that runs the negotiated codec, not from
 * the AVC decoder or the larger of AVC and HEVC: a phone may pair decoders of different families.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class MediaCodecNegotiatedDecoderTest {
    // An OMX AVC decoder: one slice, no direct submit.
    private static final String AVC_DECODER = "omx.mtk.video.decoder.avc";
    // A Codec2 HEVC decoder: four slices (c2.android), direct submit (c2.).
    private static final String HEVC_DECODER = "c2.android.hevc.decoder";

    @Before
    public void setUp() {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        MediaCodecHelper.forgetCodecList();
        ShadowMediaCodecList.addCodec(decoder(AVC_DECODER, "video/avc", CodecProfileLevel.AVCProfileHigh));
        ShadowMediaCodecList.addCodec(decoder(HEVC_DECODER, "video/hevc", CodecProfileLevel.HEVCProfileMain));
    }

    @After
    public void tearDown() {
        MediaCodecHelper.forgetCodecList();
    }

    private static MediaCodecInfo decoder(String name, String mime, int profile) {
        CodecProfileLevel level = new CodecProfileLevel();
        level.profile = profile;
        level.level = 0;
        return MediaCodecInfoBuilder.newBuilder().setName(name).setIsHardwareAccelerated(true)
                .setIsSoftwareOnly(false).setCapabilities(
                        CodecCapabilitiesBuilder.newBuilder()
                                .setMediaFormat(MediaFormat.createVideoFormat(mime, 1920, 1080))
                                .setProfileLevels(new CodecProfileLevel[] {level})
                                .setColorFormats(new int[] {CodecCapabilities.COLOR_FormatYUV420Flexible}).build())
                .build();
    }

    private static int slices(int capabilities) {
        return capabilities >>> 24;
    }

    private static boolean direct(int capabilities) {
        return (capabilities & MoonBridge.CAPABILITY_DIRECT_SUBMIT) != 0;
    }

    @Test
    public void eachNegotiatedCodecGetsItsOwnDecodersSlicesAndDirectSubmit() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
            MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity.get(), prefs,
                    null, 0, false, false, "", null);
            assertEquals(AVC_DECODER, renderer.decoderForNegotiated(MoonBridge.VIDEO_FORMAT_H264).getName());
            assertEquals(HEVC_DECODER, renderer.decoderForNegotiated(MoonBridge.VIDEO_FORMAT_H265).getName());
            // A mask runs on its best codec, as in the handshake.
            assertEquals(HEVC_DECODER, renderer.decoderForNegotiated(
                    MoonBridge.VIDEO_FORMAT_H264 | MoonBridge.VIDEO_FORMAT_H265).getName());
            assertNull(renderer.decoderForNegotiated(0));

            int avc = renderer.getCapabilities(MoonBridge.VIDEO_FORMAT_H264);
            assertEquals(1, slices(avc));
            assertFalse(direct(avc));

            // Before the fix an HEVC stream took the AVC decoder's direct submit (off here).
            int hevc = renderer.getCapabilities(MoonBridge.VIDEO_FORMAT_H265);
            assertEquals(4, slices(hevc));
            assertTrue(direct(hevc));

            // Before negotiation the old stand-ins remain: the larger slice count, AVC's direct submit.
            int unknown = renderer.getCapabilities();
            assertEquals(4, slices(unknown));
            assertFalse(direct(unknown));
        }
    }
}

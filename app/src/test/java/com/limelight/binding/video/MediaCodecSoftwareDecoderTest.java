package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.media.MediaCodecInfo;
import android.media.MediaCodecInfo.CodecCapabilities;
import android.media.MediaCodecInfo.CodecProfileLevel;
import android.media.MediaFormat;
import android.os.Build;

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
 * A software decoder (kept on emulators by SHOULD_BYPASS_SOFTWARE_BLOCK) never offers a 10-bit or
 * HDR format to the host: its 10-bit output renders corrupt.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class MediaCodecSoftwareDecoderTest {
    private static final String AVC = "video/avc";
    private static final String HEVC = "video/hevc";
    private static final String AV1 = "video/av01";

    @Before
    public void setUp() {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        MediaCodecHelper.forgetCodecList();
        ShadowMediaCodecList.addCodec(decoder("c2.test.avc.decoder", AVC, true, CodecProfileLevel.AVCProfileHigh));
    }

    @After
    public void tearDown() {
        MediaCodecHelper.forgetCodecList();
    }

    private static MediaCodecInfo decoder(String name, String mime, boolean hardware, int... profiles) {
        CodecProfileLevel[] levels = new CodecProfileLevel[profiles.length];
        for (int i = 0; i < profiles.length; i++) {
            levels[i] = new CodecProfileLevel();
            levels[i].profile = profiles[i];
            levels[i].level = 0;
        }
        return MediaCodecInfoBuilder.newBuilder().setName(name).setIsHardwareAccelerated(hardware)
                .setIsSoftwareOnly(!hardware).setCapabilities(
                        CodecCapabilitiesBuilder.newBuilder()
                                .setMediaFormat(MediaFormat.createVideoFormat(mime, 1920, 1080))
                                .setProfileLevels(levels)
                                .setColorFormats(new int[] {CodecCapabilities.COLOR_FormatYUV420Flexible}).build())
                .build();
    }

    private static MediaCodecDecoderRenderer renderer(Activity activity) {
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
        prefs.enableSdr10Bit = true;
        MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity, prefs,
                null, 0, false, false, "", null);
        renderer.setSdr10Requested(true);
        return renderer;
    }

    @Test
    public void softwareDecodersAreRecognisedByTheirFlagsOnQAndByNameBelow() {
        assertTrue(MediaCodecHelper.isSoftwareDecoder(decoder("vendor.sw", AV1, false)));
        assertFalse(MediaCodecHelper.isSoftwareDecoder(decoder("c2.vendor.av1.decoder", AV1, true)));
        // Below Android Q only the name tells.
        int sdk = Build.VERSION.SDK_INT;
        ReflectionHelpers.setStaticField(Build.VERSION.class, "SDK_INT", 28);
        try {
            assertTrue(MediaCodecHelper.isSoftwareDecoder(decoder("c2.android.av1.decoder", AV1, true)));
            assertTrue(MediaCodecHelper.isSoftwareDecoder(decoder("OMX.google.h264.decoder", AVC, true)));
            assertTrue(MediaCodecHelper.isSoftwareDecoder(decoder("c2.google.hevc.decoder", HEVC, true)));
            assertFalse(MediaCodecHelper.isSoftwareDecoder(decoder("c2.qti.av1.decoder", AV1, true)));
        } finally {
            ReflectionHelpers.setStaticField(Build.VERSION.class, "SDK_INT", sdk);
        }
    }

    @Test
    public void aSoftwareOnlyMain10DecoderOffersNo10BitFormat() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = renderer(activity.get());
            MediaCodecInfo software = decoder("c2.android.av1.decoder", AV1, false,
                    CodecProfileLevel.AV1ProfileMain8, CodecProfileLevel.AV1ProfileMain10,
                    CodecProfileLevel.AV1ProfileMain10HDR10);
            ReflectionHelpers.setField(renderer, "av1Decoder", software);
            ReflectionHelpers.setField(renderer, "av1Decoder10", software);
            assertFalse(renderer.isAv1Main10Supported());
            assertFalse(renderer.isAv1Main10SdrSupported());
            assertEquals(0, renderer.getSupportedVideoFormats(true, true) & MoonBridge.VIDEO_FORMAT_MASK_10BIT);
            // The 8-bit format is still offered.
            assertNotEquals(0, renderer.getSupportedVideoFormats(true, true) & MoonBridge.VIDEO_FORMAT_AV1_MAIN8);
        }
    }

    @Test
    public void aHardwareMain10DecoderStillOffersTheTenBitFormats() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = renderer(activity.get());
            MediaCodecInfo hardware = decoder("c2.vendor.av1.decoder", AV1, true,
                    CodecProfileLevel.AV1ProfileMain8, CodecProfileLevel.AV1ProfileMain10,
                    CodecProfileLevel.AV1ProfileMain10HDR10);
            ReflectionHelpers.setField(renderer, "av1Decoder", hardware);
            ReflectionHelpers.setField(renderer, "av1Decoder10", hardware);
            assertTrue(renderer.isAv1Main10Supported());
            assertTrue(renderer.isAv1Main10SdrSupported());
        }
    }
}

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
import org.robolectric.shadows.ShadowBuild;
import org.robolectric.shadows.ShadowMediaCodecList;
import org.robolectric.util.ReflectionHelpers;

import java.util.Arrays;
import java.util.List;
import java.util.Map;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

/**
 * Erratum 23 with Android's low-latency mode on: one reference and no long-term ones on the one
 * Fire TV's MediaTek decoders. With the mode off, on another model or on another decoder, every
 * RFI capability stays what it was.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class DecoderLowLatencyCorruptionTest {
    private static final String MODEL = "AFTNE4E43F";
    private static final int ALL_RFI = MoonBridge.CAPABILITY_REFERENCE_FRAME_INVALIDATION_AVC |
            MoonBridge.CAPABILITY_REFERENCE_FRAME_INVALIDATION_HEVC | MoonBridge.CAPABILITY_REFERENCE_FRAME_INVALIDATION_AV1;

    private ActivityController<Activity> activity;

    @Before
    public void setUp() {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "isFireTv", true);
        MediaCodecHelper.addMediaTekAndQcomRefFrameInvalidation();
        MediaCodecHelper.forgetCodecList();
        activity = Robolectric.buildActivity(Activity.class).create();
    }

    @After
    public void tearDown() {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "isFireTv", false);
        MediaCodecHelper.forgetCodecList();
        activity.close();
    }

    private static void decoders(String vendor) {
        ShadowMediaCodecList.addCodec(decoder("c2." + vendor + ".avc.decoder", "video/avc"));
        ShadowMediaCodecList.addCodec(decoder("c2." + vendor + ".hevc.decoder", "video/hevc"));
        ShadowMediaCodecList.addCodec(decoder("c2." + vendor + ".av1.decoder", "video/av01"));
    }

    private static MediaCodecInfo decoder(String name, String mime) {
        Map<String, List<String>> parameters = ReflectionHelpers.getStaticField(
                MediaCodecHelper.class, "vendorParameterCache");
        parameters.put(name, Arrays.asList("vendor.low-latency.enable"));
        MediaFormat capabilities = MediaFormat.createVideoFormat(mime, 3840, 2160);
        capabilities.setFeatureEnabled(CodecCapabilities.FEATURE_LowLatency, true);
        return MediaCodecInfoBuilder.newBuilder().setName(name).setIsHardwareAccelerated(true).setCapabilities(
                CodecCapabilitiesBuilder.newBuilder().setMediaFormat(capabilities)
                        .setProfileLevels(new CodecProfileLevel[0])
                        .setColorFormats(new int[] {CodecCapabilities.COLOR_FormatYUV420Flexible}).build()).build();
    }

    private int capabilities(PreferenceConfiguration.FormatOption format, boolean lowLatency) {
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = format;
        prefs.width = 3840;
        prefs.height = 2160;
        prefs.codecLowLatency = lowLatency;
        prefs.vendorLowLatency = true;
        return new MediaCodecDecoderRenderer(activity.get(), prefs, null, 0, false, false, "", null).getCapabilities() & ALL_RFI;
    }

    @Test
    public void lowLatencyOnTheModelsMediaTekDecodersAsksForOneReferenceOnEveryCodec() {
        ShadowBuild.setModel(MODEL);
        decoders("mtk");
        assertEquals(0, capabilities(PreferenceConfiguration.FormatOption.FORCE_HEVC, true));
        assertEquals(0, capabilities(PreferenceConfiguration.FormatOption.FORCE_AV1, true));
        assertEquals(0, capabilities(PreferenceConfiguration.FormatOption.FORCE_H264, true));
    }

    @Test
    public void tenBitSdrFollowsTheBaseDecoderWhenItHasNoneOfItsOwn() {
        ShadowBuild.setModel(MODEL);
        decoders("mtk");
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.FORCE_AV1;
        prefs.codecLowLatency = true;
        MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity.get(), prefs, null, 0, false, false, "", null);
        assertFalse(ReflectionHelpers.<Boolean>getField(renderer, "refFrameInvalidationHevc10"));
        assertFalse(ReflectionHelpers.<Boolean>getField(renderer, "refFrameInvalidationAv110"));
    }

    @Test
    public void lowLatencyOffKeepsReferenceFrameInvalidation() {
        ShadowBuild.setModel(MODEL);
        decoders("mtk");
        int hevc = capabilities(PreferenceConfiguration.FormatOption.FORCE_HEVC, false);
        assertTrue((hevc & MoonBridge.CAPABILITY_REFERENCE_FRAME_INVALIDATION_AVC) != 0);
        assertTrue((hevc & MoonBridge.CAPABILITY_REFERENCE_FRAME_INVALIDATION_HEVC) != 0);
        assertTrue((capabilities(PreferenceConfiguration.FormatOption.FORCE_AV1, false) &
                MoonBridge.CAPABILITY_REFERENCE_FRAME_INVALIDATION_AV1) != 0);
    }

    @Test
    public void anotherModelWithTheSameDecodersIsUntouched() {
        for (String model : new String[] {"AFTNE4E43", "AFTNE4E43F2", "aftne4e43f", "AFTMM"}) {
            ShadowBuild.setModel(model);
            MediaCodecHelper.forgetCodecList();
            ShadowMediaCodecList.reset();
            decoders("mtk");
            int hevc = capabilities(PreferenceConfiguration.FormatOption.FORCE_HEVC, true);
            assertTrue(model, (hevc & MoonBridge.CAPABILITY_REFERENCE_FRAME_INVALIDATION_AVC) != 0);
            assertTrue(model, (hevc & MoonBridge.CAPABILITY_REFERENCE_FRAME_INVALIDATION_HEVC) != 0);
            assertTrue(model, (capabilities(PreferenceConfiguration.FormatOption.FORCE_AV1, true) &
                    MoonBridge.CAPABILITY_REFERENCE_FRAME_INVALIDATION_AV1) != 0);
        }
    }

    @Test
    public void theModelWithAnotherDecoderIsUntouched() {
        ShadowBuild.setModel(MODEL);
        decoders("acme");
        assertTrue((capabilities(PreferenceConfiguration.FormatOption.FORCE_HEVC, true) &
                MoonBridge.CAPABILITY_REFERENCE_FRAME_INVALIDATION_HEVC) != 0);
        assertTrue((capabilities(PreferenceConfiguration.FormatOption.FORCE_AV1, true) &
                MoonBridge.CAPABILITY_REFERENCE_FRAME_INVALIDATION_AV1) != 0);
    }

    @Test
    public void aStreamRunsInTheModeOnlyWhenTheFlagReachesTheDecoder() {
        ShadowBuild.setModel(MODEL);
        int auto = PreferenceConfiguration.HEVC_LOW_LATENCY_AUTO;
        int off = PreferenceConfiguration.HEVC_LOW_LATENCY_OFF;
        String mtk = "c2.mtk.hevc.decoder";
        assertTrue(MediaCodecHelper.runsCorruptingLowLatency("video/hevc", mtk, true, auto));
        assertTrue(MediaCodecHelper.runsCorruptingLowLatency("video/hevc", mtk, true, PreferenceConfiguration.HEVC_LOW_LATENCY_ON));
        assertFalse("the switch is off", MediaCodecHelper.runsCorruptingLowLatency("video/hevc", mtk, false, auto));
        assertFalse("the HEVC override leaves the flag out", MediaCodecHelper.runsCorruptingLowLatency("video/hevc", mtk, true, off));
        assertTrue("the override is HEVC's alone", MediaCodecHelper.runsCorruptingLowLatency("video/av01", "c2.mtk.av1.decoder", true, off));
        assertFalse(MediaCodecHelper.runsCorruptingLowLatency("video/hevc", "c2.qti.hevc.decoder", true, auto));
        ShadowBuild.setModel("AFTMM");
        assertFalse(MediaCodecHelper.runsCorruptingLowLatency("video/hevc", mtk, true, auto));
    }
}

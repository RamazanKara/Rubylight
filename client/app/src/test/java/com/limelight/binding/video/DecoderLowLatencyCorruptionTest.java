package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.media.MediaCodecInfo;
import android.media.MediaCodecInfo.CodecCapabilities;
import android.media.MediaCodecInfo.CodecProfileLevel;
import android.graphics.SurfaceTexture;
import android.media.MediaFormat;
import android.view.Display;
import android.view.Surface;

import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.utils.RecentStreams;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.Shadows;
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
        shadows = {KeyframeSafetyNetWiringTest.QueueingCodec.class, com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class},
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

    private static MediaCodecInfo decoder(String name, String mime, int... profiles) {
        Map<String, List<String>> parameters = ReflectionHelpers.getStaticField(
                MediaCodecHelper.class, "vendorParameterCache");
        parameters.put(name, Arrays.asList("vendor.low-latency.enable"));
        MediaFormat capabilities = MediaFormat.createVideoFormat(mime, 3840, 2160);
        capabilities.setFeatureEnabled(CodecCapabilities.FEATURE_LowLatency, true);
        CodecProfileLevel[] levels = new CodecProfileLevel[profiles.length];
        for (int i = 0; i < profiles.length; i++) {
            levels[i] = new CodecProfileLevel();
            levels[i].profile = profiles[i];
        }
        return MediaCodecInfoBuilder.newBuilder().setName(name).setIsHardwareAccelerated(true).setCapabilities(
                CodecCapabilitiesBuilder.newBuilder().setMediaFormat(capabilities)
                        .setProfileLevels(levels)
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

    private static final int HEVC_RFI = MoonBridge.CAPABILITY_REFERENCE_FRAME_INVALIDATION_HEVC;
    private static final int AV1_RFI = MoonBridge.CAPABILITY_REFERENCE_FRAME_INVALIDATION_AV1;

    private static boolean rfi(MediaCodecDecoderRenderer renderer, int format, int hdrFormat, int capability) {
        return (renderer.getCapabilities(format, hdrFormat) & capability) != 0;
    }

    @Test
    public void aDolbyVisionDecoderOfTheModelTakesRfiFromItsOwnStreamOnly() {
        ShadowBuild.setModel(MODEL);
        decoders("acme");
        MediaCodecDecoderRenderer renderer = rendererWithDolbyVision("c2.mtk.dolby-vision.decoder", false);
        assertFalse("runs on its own decoder",
                rfi(renderer, MoonBridge.VIDEO_FORMAT_H265_MAIN10, HdrFormatPolicy.DV_81, HEVC_RFI));
        assertFalse(rfi(renderer, MoonBridge.VIDEO_FORMAT_H265_MAIN10, HdrFormatPolicy.DV_84, HEVC_RFI));
        assertFalse(rfi(renderer, MoonBridge.VIDEO_FORMAT_AV1_MAIN10, HdrFormatPolicy.DV_101, AV1_RFI));
        // The plain decoders of this test aren't the model's, so their streams keep RFI.
        assertTrue(rfi(renderer, MoonBridge.VIDEO_FORMAT_H265_MAIN10, HdrFormatPolicy.PQ, HEVC_RFI));
        assertTrue(rfi(renderer, MoonBridge.VIDEO_FORMAT_AV1_MAIN10, HdrFormatPolicy.PQ, AV1_RFI));
    }

    // Neither the picked format nor erratum 23 is why here: the override keeps HEVC from RFI's extra buffering
    // by itself (MediaCodecHelper.decoderSupportsRefFrameInvalidationHevc), so on this TV plain HEVC has none.
    @Test
    public void theHevcOverrideOffHasNoHevcRfiOfItsOwn() {
        ShadowBuild.setModel(MODEL);
        decoders("mtk");
        MediaCodecDecoderRenderer renderer = rendererWithDolbyVision("c2.mtk.dolby-vision.decoder", true);
        assertFalse(rfi(renderer, MoonBridge.VIDEO_FORMAT_H265, 0, HEVC_RFI));
        assertFalse(rfi(renderer, MoonBridge.VIDEO_FORMAT_H265_MAIN10, HdrFormatPolicy.PQ, HEVC_RFI));
        assertFalse(rfi(renderer, MoonBridge.VIDEO_FORMAT_H265_MAIN10, HdrFormatPolicy.DV_81, HEVC_RFI));
    }

    // A launch through setup to the end of the stream: what the report's row says of RFI.
    private static boolean rfiInTheRow(MediaCodecDecoderRenderer renderer, int format, int hdrFormat) {
        RecentStreams.clear();
        renderer.getCapabilities(format, hdrFormat);
        SurfaceTexture texture = new SurfaceTexture(0);
        Surface surface = new Surface(texture);
        ReflectionHelpers.setField(renderer, "renderTarget", surface);
        try {
            assertEquals(0, renderer.setup(format, 3840, 2160, 60));
        } finally {
            renderer.stop();
            renderer.cleanup();
            surface.release();
            texture.release();
        }
        String section = RecentStreams.section();
        assertTrue(section, section.contains(", RFI on") != section.contains(", RFI off"));
        return section.contains(", RFI on");
    }

    // The launch has told the host, so the report says so on whichever decoder the stream ends up.
    @Test
    public void whatTheLaunchGaveUpIsWhatTheStreamReports() {
        ShadowBuild.setModel(MODEL);
        decoders("acme");
        int hevc = MoonBridge.VIDEO_FORMAT_H265_MAIN10;
        int av1 = MoonBridge.VIDEO_FORMAT_AV1_MAIN10;
        Object[][] launches = {
                {hevc, HdrFormatPolicy.DV_81, HEVC_RFI, false},
                {hevc, HdrFormatPolicy.DV_84, HEVC_RFI, false},
                {av1, HdrFormatPolicy.DV_101, AV1_RFI, false},
                {hevc, HdrFormatPolicy.PQ, HEVC_RFI, true},
                {av1, HdrFormatPolicy.PQ, AV1_RFI, true},
                // A Dolby Vision pick for the other codec: this codec's RFI was declared, and runs.
                {av1, HdrFormatPolicy.DV_81, AV1_RFI, true},
                {hevc, HdrFormatPolicy.DV_101, HEVC_RFI, true},
        };
        for (Object[] launch : launches) {
            int format = (int) launch[0];
            int hdrFormat = (int) launch[1];
            String name = "format " + format + ", HDR format " + hdrFormat;
            MediaCodecDecoderRenderer renderer = rendererWithDolbyVision("c2.mtk.dolby-vision.decoder", false);
            assertEquals(name, launch[3], rfi(renderer, format, hdrFormat, (int) launch[2]));
            assertEquals(name, launch[3], rfiInTheRow(renderer, format, hdrFormat));
        }
    }

    // Force AV1 still offers HEVC Main10, so the launch can pick DV_81 for a decoder the stream never runs on:
    // the host takes AV1 first, and AV1's RFI was declared.
    @Test
    public void forceAv1WithAProfile8PickKeepsAv1Rfi() {
        ShadowBuild.setModel(MODEL);
        ShadowMediaCodecList.addCodec(decoder("c2.acme.hevc.decoder", "video/hevc", CodecProfileLevel.HEVCProfileMain10));
        ShadowMediaCodecList.addCodec(decoder("c2.acme.av1.decoder", "video/av01", CodecProfileLevel.AV1ProfileMain10));
        ShadowMediaCodecList.addCodec(decoder("c2.mtk.dolby-vision.decoder", HdrFormatPolicy.MIME_DOLBY_VISION,
                CodecProfileLevel.DolbyVisionProfileDvheSt));
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.FORCE_AV1;
        prefs.width = 3840;
        prefs.height = 2160;
        prefs.codecLowLatency = true;
        prefs.vendorLowLatency = true;
        MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity.get(), prefs, null, 0, false, true, "", null);
        Display display = activity.get().getWindowManager().getDefaultDisplay();
        Shadows.shadowOf(display).setDisplayHdrCapabilities(display.getDisplayId(), 1000, 500, 0.0005f,
                Display.HdrCapabilities.HDR_TYPE_DOLBY_VISION);
        ReflectionHelpers.setField(renderer, "outputDisplay", display);

        assertTrue("the offer carries HEVC", renderer.isHevcMain10Supported());
        int offered = MoonBridge.VIDEO_FORMAT_H265_MAIN10 | MoonBridge.VIDEO_FORMAT_AV1_MAIN10;
        int hdrFormat = renderer.pickHdrFormat(HdrFormatPolicy.DV_81 | HdrFormatPolicy.PQ, offered, false, false);
        assertEquals(HdrFormatPolicy.DV_81, hdrFormat);
        assertEquals("only the automatic codec moves", offered, renderer.videoFormatsForHdrFormat(hdrFormat, offered));
        assertTrue("declared", rfi(renderer, MoonBridge.VIDEO_FORMAT_AV1_MAIN10, hdrFormat, AV1_RFI));
        assertTrue("and reported", rfiInTheRow(renderer, MoonBridge.VIDEO_FORMAT_AV1_MAIN10, hdrFormat));
    }

    @Test
    public void aDolbyVisionDecoderOfAnotherModelTakesNothing() {
        ShadowBuild.setModel("AFTMM");
        decoders("acme");
        MediaCodecDecoderRenderer renderer = rendererWithDolbyVision("c2.mtk.dolby-vision.decoder", false);
        assertTrue(rfi(renderer, MoonBridge.VIDEO_FORMAT_H265_MAIN10, HdrFormatPolicy.DV_81, HEVC_RFI));
        assertTrue(rfi(renderer, MoonBridge.VIDEO_FORMAT_AV1_MAIN10, HdrFormatPolicy.DV_101, AV1_RFI));
    }

    @Test
    public void aFormatAsksForTheModeOnlyWithTheFlagSet() {
        MediaFormat format = MediaFormat.createVideoFormat("video/hevc", 3840, 2160);
        assertFalse(MediaCodecHelper.asksForAndroidLowLatency(format));
        format.setInteger(MediaFormat.KEY_LOW_LATENCY, 0);
        assertFalse(MediaCodecHelper.asksForAndroidLowLatency(format));
        format.setInteger(MediaFormat.KEY_LOW_LATENCY, 1);
        assertTrue(MediaCodecHelper.asksForAndroidLowLatency(format));
    }

    private MediaCodecDecoderRenderer rendererWithDolbyVision(String decoderName, boolean hevcOverrideOff) {
        ShadowMediaCodecList.addCodec(decoder(decoderName, HdrFormatPolicy.MIME_DOLBY_VISION,
                CodecProfileLevel.DolbyVisionProfileDvheSt, CodecProfileLevel.DolbyVisionProfileDvav110));
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
        prefs.width = 3840;
        prefs.height = 2160;
        prefs.codecLowLatency = true;
        prefs.vendorLowLatency = true;
        prefs.disableHevcLowLatency = hevcOverrideOff;
        return new MediaCodecDecoderRenderer(activity.get(), prefs, null, 0, false, /* requestedHdr */ true, "", null);
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

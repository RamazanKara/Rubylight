package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.media.MediaCodec;
import android.media.MediaCodecInfo;
import android.media.MediaCodecInfo.CodecCapabilities;
import android.media.MediaCodecInfo.CodecProfileLevel;
import android.media.MediaFormat;
import android.os.Bundle;

import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

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
import org.robolectric.shadows.MediaCodecInfoBuilder.CodecCapabilitiesBuilder;
import org.robolectric.shadows.ShadowBuild;
import org.robolectric.shadows.ShadowMediaCodec;
import org.robolectric.shadows.ShadowMediaCodecList;
import org.robolectric.util.ReflectionHelpers;
import org.robolectric.util.ReflectionHelpers.ClassParameter;

import java.util.Arrays;
import java.util.List;
import java.util.Map;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class)
public class MediaCodecHevcLowLatencyTest {
    private static final String HEVC = "video/hevc";
    private static final String AML_HEVC = "c2.amlogic.hevc.decoder";
    private static final String AML_LOW_LATENCY = "vendor.low-latency.enable";
    private static final String OTHER_HEVC = "c2.acme.hevc.decoder";

    @Before
    public void setUp() {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
    }

    @Test
    @Config(sdk = {28, 34, 35})
    public void optInOmitsEveryLowLatencyFlagOnAnyHevcDecoder() {
        for (String name : new String[] {AML_HEVC, "omx.amlogic.hevc.decoder", "c2.qti.hevc.decoder",
                "c2.mtk.hevc.decoder", "c2.exynos.hevc.decoder"}) {
            MediaCodecInfo decoder = decoder(name, HEVC);
            for (int attempt = 0; attempt <= 5; attempt++) {
                MediaFormat format = MediaFormat.createVideoFormat(HEVC, 1920, 1080);
                assertFalse(MediaCodecHelper.setDecoderLowLatencyOptions(format, decoder, attempt,
                        true, true, false, true));
                assertFalse(name, format.containsKey(MediaFormat.KEY_LOW_LATENCY));
                assertFalse(name, format.containsKey("vdec-lowlatency"));
                for (String key : VendorLowLatencyKeys.KNOWN.keySet()) {
                    assertFalse(name + ": " + key, format.containsKey(key));
                }
                assertEquals(1920, format.getInteger(MediaFormat.KEY_WIDTH));
                assertEquals(1080, format.getInteger(MediaFormat.KEY_HEIGHT));
            }
        }
    }

    @Test
    @Config(sdk = {28, 34})
    public void amlogicHevcDecodersNeverGetTheFlagsWithoutTheOptIn() {
        for (String name : new String[] {AML_HEVC, "OMX.amlogic.hevc.decoder.awesome2"}) {
            for (String model : new String[] {"MiTV-AYFR0", "Homatics Box 4K Pro V2", "Chromecast", "AFTMM"}) {
                ShadowBuild.setModel(model);
                MediaCodecInfo decoder = decoder(name, HEVC);
                for (int attempt = 0; attempt <= 5; attempt++) {
                    MediaFormat format = MediaFormat.createVideoFormat(HEVC, 1920, 1080);
                    MediaCodecHelper.setDecoderLowLatencyOptions(format, decoder, attempt, true, true, false, false);
                    assertFalse(model, format.containsKey(MediaFormat.KEY_LOW_LATENCY));
                    assertFalse(model, format.containsKey(AML_LOW_LATENCY));
                    assertFalse(model, format.containsKey("vdec-lowlatency"));
                }
                // Decoder performance hints keep their own setting, as with the opt-in.
                assertEquals(model, 0, tunedFormat(name, HEVC, false).getInteger(MediaFormat.KEY_PRIORITY));
            }
            // Their H.264 and AV1 decoders keep the flags.
            assertEquals(1, tunedFormat("c2.amlogic.avc.decoder", "video/avc", false).getInteger(AML_LOW_LATENCY));
            assertEquals(1, tunedFormat("c2.amlogic.av1.decoder", "video/av01", false).getInteger(MediaFormat.KEY_LOW_LATENCY));
        }
        // HEVC RFI keeps its normal policy (erratum 22): referenceFrameInvalidationUses... below.
    }

    @Test
    public void otherHevcDecodersKeepTheFlagsWithoutTheOptIn() {
        for (String name : new String[] {OTHER_HEVC, "c2.qti.hevc.decoder", "c2.exynos.hevc.decoder"}) {
            MediaFormat format = tunedFormat(name, HEVC, false);
            assertEquals(name, 1, format.getInteger(MediaFormat.KEY_LOW_LATENCY));
        }
        assertEquals(1, tunedFormat(OTHER_HEVC, HEVC, false).getInteger(AML_LOW_LATENCY));
    }

    @Test
    public void h264AndAv1KeepTheirStandardAndVendorOptionsWhenHevcFlagsAreDisabled() {
        String[] mimes = {"video/avc", "video/av01"};
        String[] names = {"c2.amlogic.avc.decoder", "c2.amlogic.av1.decoder"};
        for (int i = 0; i < mimes.length; i++) {
            MediaFormat format = tunedFormat(names[i], mimes[i], true);
            assertEquals(1, format.getInteger(MediaFormat.KEY_LOW_LATENCY));
            assertEquals(1, format.getInteger(AML_LOW_LATENCY));
        }
    }

    @Test
    public void performanceHintsKeepTheirSettingAndCanFallBackToAnUntunedFormat() {
        MediaCodecInfo decoder = decoder(AML_HEVC, HEVC);
        MediaFormat first = tunedFormat(AML_HEVC, HEVC, true);
        assertFalse(first.containsKey(MediaFormat.KEY_LOW_LATENCY));
        assertFalse(first.containsKey(AML_LOW_LATENCY));
        assertEquals(0, first.getInteger(MediaFormat.KEY_PRIORITY));
        for (int attempt = 1; attempt <= 5; attempt++) {
            MediaFormat format = MediaFormat.createVideoFormat(HEVC, 1920, 1080);
            assertFalse(MediaCodecHelper.setDecoderLowLatencyOptions(format, decoder, attempt,
                    true, true, true, true));
            assertFalse(format.containsKey(MediaFormat.KEY_PRIORITY));
            assertFalse(format.containsKey(MediaFormat.KEY_LOW_LATENCY));
            assertFalse(format.containsKey(AML_LOW_LATENCY));
        }
    }

    @Test
    public void referenceFrameInvalidationUsesEnabledLatencyOptionsAndHonorsTheOverride() {
        MediaCodecInfo decoder = decoder(AML_HEVC, HEVC);
        assertTrue(MediaCodecHelper.decoderSupportsRefFrameInvalidationHevc(decoder, true, false, false));
        assertTrue(MediaCodecHelper.decoderSupportsRefFrameInvalidationHevc(decoder, false, true, false));
        assertFalse(MediaCodecHelper.decoderSupportsRefFrameInvalidationHevc(decoder, false, false, false));
        assertFalse(MediaCodecHelper.decoderSupportsRefFrameInvalidationHevc(decoder, true, true, true));
        // Explicitly verified decoders keep their normal RFI policy unless the stability override is on.
        MediaCodecInfo verified = decoder("c2.exynos.hevc.decoder", HEVC);
        assertTrue(MediaCodecHelper.decoderSupportsRefFrameInvalidationHevc(verified, false, false, false));
        assertFalse(MediaCodecHelper.decoderSupportsRefFrameInvalidationHevc(verified, true, true, true));
    }

    @Test @Config(sdk = 25)
    public void fireTv3KeepsItsRequiredHevcFlagEvenWhenTuningIsDisabledOrFallsBack() {
        ShadowBuild.setManufacturer("Amazon");
        ShadowBuild.setModel("AFTN");
        MediaCodecInfo decoder = decoder("OMX.amlogic.hevc.decoder", HEVC);
        for (boolean override : new boolean[] {false, true}) {
            for (int attempt = 0; attempt <= 5; attempt++) {
                MediaFormat format = MediaFormat.createVideoFormat(HEVC, 1920, 1080);
                boolean tuned = MediaCodecHelper.setDecoderLowLatencyOptions(format, decoder, attempt,
                        false, false, false, override);
                assertEquals(attempt == 0, tuned); // The final fallback must still be terminal.
                assertEquals(1, format.getInteger("vdec-lowlatency"));
                assertFalse(format.containsKey(MediaFormat.KEY_LOW_LATENCY));
            }
        }
        MediaFormat avc = MediaFormat.createVideoFormat("video/avc", 1920, 1080);
        assertFalse(MediaCodecHelper.setDecoderLowLatencyOptions(avc, decoder("omx.amlogic.avc.decoder", "video/avc"),
                0, false, false, false, true));
        assertFalse(avc.containsKey("vdec-lowlatency"));
        ShadowBuild.setModel("AFTA");
        MediaFormat otherFireTv = MediaFormat.createVideoFormat(HEVC, 1920, 1080);
        assertFalse(MediaCodecHelper.setDecoderLowLatencyOptions(otherFireTv, decoder, 0, true, true, false, true));
        assertFalse(otherFireTv.containsKey("vdec-lowlatency"));
        ShadowBuild.setModel("AFTN");
        ShadowBuild.setManufacturer("Xiaomi");
        MediaFormat other = MediaFormat.createVideoFormat(HEVC, 1920, 1080);
        assertFalse(MediaCodecHelper.setDecoderLowLatencyOptions(other, decoder, 0, true, true, false, true));
        assertFalse(other.containsKey("vdec-lowlatency"));
    }

    @Test @Config(sdk = {28, 34})
    public void newerFireOsDoesNotForceLegacyAmlogicCompatibilityFlags() {
        ShadowBuild.setManufacturer("Amazon");
        ShadowBuild.setModel("AFTN");
        MediaFormat format = MediaFormat.createVideoFormat(HEVC, 1920, 1080);
        assertFalse(MediaCodecHelper.setDecoderLowLatencyOptions(format,
                decoder("omx.amlogic.hevc.decoder", HEVC), 0, true, true, false, true));
        assertFalse(format.containsKey("vdec-lowlatency"));
    }

    @Test
    @Config(shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
            instrumentedPackages = "com.limelight.nvstream.jni")
    public void negotiationDoesNotAdvertiseHevcRfiWhenTheOverrideDisablesIt() {
        ShadowMediaCodecList.addCodec(decoder(AML_HEVC, HEVC));
        for (boolean disable : new boolean[] {false, true}) {
            try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
                PreferenceConfiguration prefs = new PreferenceConfiguration();
                prefs.videoFormat = PreferenceConfiguration.FormatOption.FORCE_HEVC;
                prefs.codecLowLatency = prefs.vendorLowLatency = true;
                prefs.disableHevcLowLatency = disable;
                MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity.get(), prefs,
                        null, 0, false, false, "", null);
                assertNotNull(ReflectionHelpers.getField(renderer, "hevcDecoder"));
                assertEquals(!disable, (renderer.getCapabilities() & MoonBridge.CAPABILITY_REFERENCE_FRAME_INVALIDATION_HEVC) != 0);
            }
        }
    }

    @Test
    @Config(shadows = RecordingCodec.class)
    public void streamStartupAndRestartApplyTheOptInWithoutReassertingDisabledFlags() {
        // The opt-in on another decoder, no opt-in there, and an Amlogic decoder without the opt-in.
        String[] names = {OTHER_HEVC, OTHER_HEVC, AML_HEVC};
        boolean[] optIn = {true, false, false};
        for (int run = 0; run < names.length; run++) {
            RecordingCodec.name = names[run];
            boolean disable = optIn[run] || names[run].equals(AML_HEVC);
            try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
                PreferenceConfiguration prefs = new PreferenceConfiguration();
                prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
                prefs.codecLowLatency = prefs.vendorLowLatency = prefs.codecPerformance = true;
                prefs.disableHevcLowLatency = optIn[run];
                MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity.get(), prefs,
                        null, 0, false, false, "", null);
                ReflectionHelpers.setField(renderer, "videoFormat", MoonBridge.VIDEO_FORMAT_H265);
                ReflectionHelpers.setField(renderer, "hevcDecoder", decoder(names[run], HEVC));
                // The Amlogic skip leaves RFI to its own policy; only the opt-in turns it off.
                ReflectionHelpers.setField(renderer, "refFrameInvalidationHevc", !optIn[run]);
                ReflectionHelpers.setField(renderer, "initialWidth", 1920);
                ReflectionHelpers.setField(renderer, "initialHeight", 1080);
                ReflectionHelpers.setField(renderer, "refreshRate", 60);
                RecordingCodec.runtimeParameterCalls = 0;
                try {
                    assertEquals(0, renderer.initializeDecoder(true));
                    assertEquals(disable ? 1 : 2, (int) ReflectionHelpers.getField(renderer, "outputBufferQueueLimit"));
                    assertEquals(!optIn[run], (boolean) ReflectionHelpers.getField(renderer, "refFrameInvalidationActive"));
                    MediaFormat format = ReflectionHelpers.getField(renderer, "configuredFormat");
                    assertEquals(!disable, format.containsKey(MediaFormat.KEY_LOW_LATENCY));
                    assertEquals(!disable, format.containsKey(AML_LOW_LATENCY));
                    assertEquals(disable ? "off" : "android+vendor", MediaCodecHelper.getDecoderLowLatencyOptions(
                            format, null, Arrays.asList(AML_LOW_LATENCY), true)[2]);
                    assertEquals(disable ? 0 : 1, RecordingCodec.runtimeParameterCalls);

                    MediaCodec codec = ReflectionHelpers.getField(renderer, "videoDecoder");
                    codec.stop();
                    ReflectionHelpers.callInstanceMethod(renderer, "configureAndStartDecoder",
                            ClassParameter.from(MediaFormat.class, format));
                    assertEquals(disable ? 0 : 2, RecordingCodec.runtimeParameterCalls);
                    assertEquals(!disable, format.containsKey(MediaFormat.KEY_LOW_LATENCY));
                    assertEquals(!disable, format.containsKey(AML_LOW_LATENCY));
                } finally {
                    renderer.stop();
                    renderer.cleanup();
                }
            }
        }
    }

    private static MediaFormat tunedFormat(String name, String mime, boolean disableHevcLowLatency) {
        MediaFormat format = MediaFormat.createVideoFormat(mime, 1920, 1080);
        assertTrue(MediaCodecHelper.setDecoderLowLatencyOptions(format, decoder(name, mime), 0,
                true, true, true, disableHevcLowLatency));
        return format;
    }

    private static MediaCodecInfo decoder(String name, String mime) {
        // Keep the policy tests independent of actual codecs exposed by the test machine.
        Map<String, List<String>> parameters = ReflectionHelpers.getStaticField(
                MediaCodecHelper.class, "vendorParameterCache");
        parameters.put(name, Arrays.asList(AML_LOW_LATENCY));
        MediaFormat capabilities = MediaFormat.createVideoFormat(mime, 1920, 1080);
        capabilities.setFeatureEnabled(CodecCapabilities.FEATURE_LowLatency, true);
        return MediaCodecInfoBuilder.newBuilder().setName(name).setCapabilities(
                CodecCapabilitiesBuilder.newBuilder().setMediaFormat(capabilities)
                        .setProfileLevels(new CodecProfileLevel[0])
                        .setColorFormats(new int[] {CodecCapabilities.COLOR_FormatYUV420Flexible}).build()).build();
    }

    @Implements(MediaCodec.class)
    public static class RecordingCodec extends ShadowMediaCodec {
        static int runtimeParameterCalls;
        static String name = AML_HEVC;

        @Implementation public String getName() {
            return name;
        }

        @Implementation public MediaCodecInfo getCodecInfo() {
            return decoder(name, HEVC);
        }

        @Implementation public void setParameters(Bundle params) {
            runtimeParameterCalls++;
        }
    }
}

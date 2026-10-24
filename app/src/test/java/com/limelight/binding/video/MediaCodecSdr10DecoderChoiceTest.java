package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.media.MediaCodecInfo;
import android.media.MediaCodecInfo.CodecCapabilities;
import android.media.MediaCodecInfo.CodecProfileLevel;
import android.media.MediaFormat;

import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

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
 * With 10-bit SDR on, every 8-bit outcome must be exactly what the setting off gives: the decoders
 * chosen for 10-bit SDR (a non-whitelisted one is accepted if it lists Main10) serve only 10-bit formats.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class MediaCodecSdr10DecoderChoiceTest {
    private static final String AVC = "video/avc";
    private static final String HEVC = "video/hevc";
    private static final String AVC_DECODER = "c2.test.avc.decoder";
    // Whitelisted by its name, 8-bit only.
    private static final String EIGHT_BIT_HEVC = "c2.test.hevc.decoder";
    // Not whitelisted (no low latency, a name the list does not know), but lists Main10.
    private static final String MAIN10_HEVC = "vendorx.hevc.main10.decoder";

    private interface Check {
        void run(ActivityController<Activity> activity);
    }

    @Before
    public void setUp() {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        ShadowMediaCodecList.addCodec(decoder(AVC_DECODER, AVC, CodecProfileLevel.AVCProfileHigh));
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

    private static MediaCodecDecoderRenderer renderer(Activity activity, boolean setting) {
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
        prefs.enableSdr10Bit = setting;
        MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity, prefs,
                null, 0, false, false, "", null);
        renderer.setSdr10Requested(setting);
        return renderer;
    }

    private static MediaCodecInfo field(MediaCodecDecoderRenderer renderer, String name) {
        return ReflectionHelpers.getField(renderer, name);
    }

    private static String name(MediaCodecInfo decoder) {
        return decoder == null ? null : decoder.getName();
    }

    private static void withRenderers(Check check) {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            check.run(activity);
        }
    }

    @Test
    public void aWhitelisted8BitDecoderKeepsEightBitAndATrialDecoderServesOnlyTenBit() {
        ShadowMediaCodecList.addCodec(decoder(EIGHT_BIT_HEVC, HEVC, CodecProfileLevel.HEVCProfileMain));
        ShadowMediaCodecList.addCodec(decoder(MAIN10_HEVC, HEVC, CodecProfileLevel.HEVCProfileMain10));
        withRenderers(activity -> {
            MediaCodecDecoderRenderer off = renderer(activity.get(), false);
            MediaCodecDecoderRenderer on = renderer(activity.get(), true);

            // The setting off uses the whitelisted decoder.
            assertEquals(EIGHT_BIT_HEVC, name(field(off, "hevcDecoder")));
            assertNull(field(off, "hevcDecoder10"));
            // With it on, the 8-bit decoders and formats are the same...
            assertEquals(EIGHT_BIT_HEVC, name(field(on, "hevcDecoder")));
            assertEquals(name(field(off, "avcDecoder")), name(field(on, "avcDecoder")));
            assertEquals(name(field(off, "av1Decoder")), name(field(on, "av1Decoder")));
            int eightBit = off.getSupportedVideoFormats(false, false);
            assertEquals(eightBit, on.getSupportedVideoFormats(false, false));
            int offered = on.getSupportedVideoFormats(false, true);
            assertEquals(eightBit, offered & ~MoonBridge.VIDEO_FORMAT_MASK_10BIT);
            // ...and only the Main10 bit is added, from the trial decoder.
            assertEquals(MoonBridge.VIDEO_FORMAT_H265_MAIN10, offered & MoonBridge.VIDEO_FORMAT_MASK_10BIT);
            assertEquals(MAIN10_HEVC, name(field(on, "hevcDecoder10")));
            // Each negotiated format picks its own decoder.
            assertEquals(EIGHT_BIT_HEVC, name(on.decoderFor(MoonBridge.VIDEO_FORMAT_H265)));
            assertEquals(MAIN10_HEVC, name(on.decoderFor(MoonBridge.VIDEO_FORMAT_H265_MAIN10)));
            assertEquals(AVC_DECODER, name(on.decoderFor(MoonBridge.VIDEO_FORMAT_H264)));
            assertEquals(name(off.decoderFor(MoonBridge.VIDEO_FORMAT_H265)),
                    name(on.decoderFor(MoonBridge.VIDEO_FORMAT_H265)));
            // A stream that was not asked for as 10-bit SDR never uses the trial decoder.
            on.setSdr10Requested(false);
            assertEquals(EIGHT_BIT_HEVC, name(on.decoderFor(MoonBridge.VIDEO_FORMAT_H265_MAIN10)));
        });
    }

    @Test
    public void aTrialDecoderAloneOffersOnlyItsTenBitFormat() {
        // The only HEVC decoder is not whitelisted: the setting off has no HEVC at all, and so has any 8-bit
        // outcome with it on; the 10-bit format alone is what the trial decoder adds.
        ShadowMediaCodecList.addCodec(decoder(MAIN10_HEVC, HEVC, CodecProfileLevel.HEVCProfileMain10));
        withRenderers(activity -> {
            MediaCodecDecoderRenderer off = renderer(activity.get(), false);
            MediaCodecDecoderRenderer on = renderer(activity.get(), true);

            assertNull(field(off, "hevcDecoder"));
            assertNull(field(on, "hevcDecoder"));
            assertFalse(on.isHevcSupported());
            int eightBit = off.getSupportedVideoFormats(false, false);
            assertEquals(0, eightBit & MoonBridge.VIDEO_FORMAT_MASK_H265);
            int offered = on.getSupportedVideoFormats(false, true);
            assertEquals(eightBit, offered & ~MoonBridge.VIDEO_FORMAT_MASK_10BIT);
            assertEquals(MoonBridge.VIDEO_FORMAT_H265_MAIN10, offered & MoonBridge.VIDEO_FORMAT_MASK_10BIT);
            assertNull(on.decoderFor(MoonBridge.VIDEO_FORMAT_H265));
            assertEquals(MAIN10_HEVC, name(on.decoderFor(MoonBridge.VIDEO_FORMAT_H265_MAIN10)));
        });
    }

    @Test
    public void aDeviceWithoutAMain10DecoderOffersNoTenBitFormats() {
        ShadowMediaCodecList.addCodec(decoder(EIGHT_BIT_HEVC, HEVC, CodecProfileLevel.HEVCProfileMain));
        withRenderers(activity -> {
            MediaCodecDecoderRenderer on = renderer(activity.get(), true);
            assertNull(field(on, "hevcDecoder10"));
            assertEquals(0, on.getSupportedVideoFormats(false, true) & MoonBridge.VIDEO_FORMAT_MASK_10BIT);
            assertFalse(on.isHevcMain10Supported());
        });
    }
}

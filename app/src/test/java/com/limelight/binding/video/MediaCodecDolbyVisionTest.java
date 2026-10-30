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
 * The Dolby Vision decoders of a TCL C6K (MediaTek MT9689): profile 8 for HEVC streams and
 * profile 10 for AV1 streams, each taken only for its own codec.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class MediaCodecDolbyVisionTest {
    private static final String PROFILE_8 = "c2.mtk.dvhe.st.decoder";
    private static final String PROFILE_10 = "c2.mtk.dvav1.10.decoder";

    @Before
    public void setUp() {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        MediaCodecHelper.forgetCodecList();
        ShadowMediaCodecList.addCodec(decoder("c2.mtk.avc.decoder", "video/avc", CodecProfileLevel.AVCProfileHigh));
        ShadowMediaCodecList.addCodec(decoder("c2.mtk.hevc.decoder", "video/hevc", CodecProfileLevel.HEVCProfileMain10));
        ShadowMediaCodecList.addCodec(decoder("c2.mtk.av1.decoder", "video/av01", CodecProfileLevel.AV1ProfileMain10));
        ShadowMediaCodecList.addCodec(decoder(PROFILE_8, HdrFormatPolicy.MIME_DOLBY_VISION,
                CodecProfileLevel.DolbyVisionProfileDvheSt));
        ShadowMediaCodecList.addCodec(decoder(PROFILE_10, HdrFormatPolicy.MIME_DOLBY_VISION,
                CodecProfileLevel.DolbyVisionProfileDvav110));
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
                                .setMediaFormat(MediaFormat.createVideoFormat(mime, 3840, 2160))
                                .setProfileLevels(new CodecProfileLevel[] {level})
                                .setColorFormats(new int[] {CodecCapabilities.COLOR_FormatYUV420Flexible}).build())
                .build();
    }

    private static MediaCodecDecoderRenderer renderer(Activity activity) {
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
        return new MediaCodecDecoderRenderer(activity, prefs, null, 0, false, true, "", null);
    }

    private static String name(MediaCodecInfo decoder) {
        return decoder == null ? null : decoder.getName();
    }

    /** The Dolby Vision decoder the next decoder of {@code videoFormat} is, for an ANNOUNCE of {@code hdrFormat}. */
    private static String decoderFor(MediaCodecDecoderRenderer renderer, int videoFormat, int hdrFormat) {
        ReflectionHelpers.setField(renderer, "videoFormat", videoFormat);
        renderer.setAnnouncedHdrFormat(hdrFormat);
        return name(ReflectionHelpers.callInstanceMethod(renderer, "dolbyVisionDecoderForStream"));
    }

    @Test
    public void eachProfileIsLookedForByItsOwnProfile() {
        assertEquals(PROFILE_8, name(MediaCodecHelper.findDolbyVisionDecoder(CodecProfileLevel.DolbyVisionProfileDvheSt)));
        assertEquals(PROFILE_10, name(MediaCodecHelper.findDolbyVisionDecoder(CodecProfileLevel.DolbyVisionProfileDvav110)));
        assertNull(MediaCodecHelper.findDolbyVisionDecoder(CodecProfileLevel.DolbyVisionProfileDvheStn));
        assertEquals("8", MediaCodecHelper.dolbyVisionProfileName(CodecProfileLevel.DolbyVisionProfileDvheSt));
        assertEquals("10", MediaCodecHelper.dolbyVisionProfileName(CodecProfileLevel.DolbyVisionProfileDvav110));
    }

    @Test
    public void anHdrRendererFindsBothAndEachDecodesOnlyItsCodec() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = renderer(activity.get());
            assertEquals(PROFILE_8, name(ReflectionHelpers.getField(renderer, "dolbyVisionDecoder")));
            assertEquals(PROFILE_10, name(ReflectionHelpers.getField(renderer, "dolbyVisionAv1Decoder")));

            int hevc = MoonBridge.VIDEO_FORMAT_H265_MAIN10;
            int av1 = MoonBridge.VIDEO_FORMAT_AV1_MAIN10;
            assertEquals(PROFILE_10, decoderFor(renderer, av1, HdrFormatPolicy.DV_101));
            assertEquals(PROFILE_8, decoderFor(renderer, hevc, HdrFormatPolicy.DV_81));
            assertEquals(PROFILE_8, decoderFor(renderer, hevc, HdrFormatPolicy.DV_84));
            // A profile whose codec the handshake did not take, 8-bit, or no Dolby Vision asked for.
            assertNull(decoderFor(renderer, av1, HdrFormatPolicy.DV_81));
            assertNull(decoderFor(renderer, hevc, HdrFormatPolicy.DV_101));
            assertNull(decoderFor(renderer, MoonBridge.VIDEO_FORMAT_AV1_MAIN8, HdrFormatPolicy.DV_101));
            assertNull(decoderFor(renderer, av1, HdrFormatPolicy.PQ));
            assertNull(decoderFor(renderer, av1, 0));

            // After a failure the same stream goes to the AV1 decoder, until the next ANNOUNCE.
            decoderFor(renderer, av1, HdrFormatPolicy.DV_101);
            ReflectionHelpers.setField(renderer, "dolbyVisionFailed", true);
            assertNull(name(ReflectionHelpers.callInstanceMethod(renderer, "dolbyVisionDecoderForStream")));
            assertEquals(PROFILE_10, decoderFor(renderer, av1, HdrFormatPolicy.DV_101));
        }
    }

    @Test
    public void theDolbyVisionFormatNamesTheProfileOfTheStreamsCodec() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = renderer(activity.get());
            for (int[] row : new int[][] {
                    {MoonBridge.VIDEO_FORMAT_AV1_MAIN10, CodecProfileLevel.DolbyVisionProfileDvav110},
                    {MoonBridge.VIDEO_FORMAT_H265_MAIN10, CodecProfileLevel.DolbyVisionProfileDvheSt}}) {
                ReflectionHelpers.setField(renderer, "videoFormat", row[0]);
                MediaFormat format = ReflectionHelpers.callInstanceMethod(renderer, "createBaseMediaFormat",
                        ReflectionHelpers.ClassParameter.from(String.class, HdrFormatPolicy.MIME_DOLBY_VISION));
                assertEquals(HdrFormatPolicy.MIME_DOLBY_VISION, format.getString(MediaFormat.KEY_MIME));
                assertEquals(row[1], format.getInteger(MediaFormat.KEY_PROFILE));
            }
            // The AV1 decoder itself gets no profile, as before.
            ReflectionHelpers.setField(renderer, "videoFormat", MoonBridge.VIDEO_FORMAT_AV1_MAIN10);
            MediaFormat av1 = ReflectionHelpers.callInstanceMethod(renderer, "createBaseMediaFormat",
                    ReflectionHelpers.ClassParameter.from(String.class, "video/av01"));
            assertFalse(av1.containsKey(MediaFormat.KEY_PROFILE));
        }
    }
}

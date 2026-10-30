package com.limelight.binding.video;

import android.app.Application;
import android.media.MediaCodecInfo;
import android.media.MediaCodecInfo.CodecCapabilities;
import android.media.MediaCodecInfo.CodecProfileLevel;
import android.media.MediaFormat;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.MediaCodecInfoBuilder;
import org.robolectric.shadows.MediaCodecInfoBuilder.CodecCapabilitiesBuilder;
import org.robolectric.shadows.ShadowMediaCodecList;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

/**
 * The platform's codec list is read once per process: building it is a blocking call to the media
 * service, which stalled the settings screen into an ANR on a MediaTek device.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class MediaCodecListCacheTest {
    @Before
    public void setUp() {
        MediaCodecHelper.forgetCodecList();
    }

    @After
    public void tearDown() {
        MediaCodecHelper.forgetCodecList();
    }

    private static MediaCodecInfo decoder(String name, String mime) {
        CodecProfileLevel level = new CodecProfileLevel();
        return MediaCodecInfoBuilder.newBuilder().setName(name).setIsHardwareAccelerated(true)
                .setIsSoftwareOnly(false).setCapabilities(
                        CodecCapabilitiesBuilder.newBuilder()
                                .setMediaFormat(MediaFormat.createVideoFormat(mime, 1920, 1080))
                                .setProfileLevels(new CodecProfileLevel[] {level})
                                .setColorFormats(new int[] {CodecCapabilities.COLOR_FormatYUV420Flexible}).build())
                .build();
    }

    @Test
    public void theCodecListIsReadOnceUntilForgotten() throws Exception {
        ShadowMediaCodecList.addCodec(decoder("c2.test.avc.decoder", "video/avc"));
        assertTrue(MediaCodecHelper.dumpDecoders().contains("c2.test.avc.decoder"));

        // The platform's list is gone (and its own cache with it): the first read is still used.
        ShadowMediaCodecList.reset();
        assertTrue(MediaCodecHelper.dumpDecoders().contains("c2.test.avc.decoder"));

        MediaCodecHelper.forgetCodecList();
        assertFalse(MediaCodecHelper.dumpDecoders().contains("c2.test.avc.decoder"));
    }
}

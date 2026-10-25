package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.media.MediaFormat;

import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;
import org.robolectric.util.ReflectionHelpers.ClassParameter;

import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.Assert.*;

/** A 10-bit SDR stream starts as SDR: the PC's first report must not restart a decoder that is already right. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class MediaCodecSdr10Test {
    private static final byte[] METADATA = new byte[24];

    private interface Check {
        void run(MediaCodecDecoderRenderer renderer);
    }

    private static void withRenderer(boolean sdr10, Check check) {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
            MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity.get(), prefs,
                    null, 0, false, false, "", null);
            renderer.setSdr10Requested(sdr10);
            ReflectionHelpers.setField(renderer, "videoFormat", MoonBridge.VIDEO_FORMAT_H265_MAIN10);
            check.run(renderer);
        }
    }

    private static int recovery(MediaCodecDecoderRenderer renderer) {
        return ((AtomicInteger) ReflectionHelpers.getField(renderer, "codecRecoveryType")).get();
    }

    private static int constant(String name) {
        return ReflectionHelpers.getStaticField(MediaCodecDecoderRenderer.class, name);
    }

    private static MediaFormat baseFormat(MediaCodecDecoderRenderer renderer) {
        return ReflectionHelpers.callInstanceMethod(renderer, "createBaseMediaFormat",
                ClassParameter.from(String.class, "video/hevc"));
    }

    @Test
    public void aFirstSdrReportDoesNotRestartATenBitSdrStream() {
        withRenderer(true, renderer -> {
            renderer.setHdrMode(false, null);
            assertEquals(constant("CR_RECOVERY_TYPE_NONE"), recovery(renderer));
            assertEquals(Boolean.FALSE, renderer.getHostHdrReport());
        });
    }

    @Test
    public void aFirstHdrReportRestartsATenBitSdrStream() {
        withRenderer(true, renderer -> {
            renderer.setHdrMode(true, METADATA);
            assertEquals(constant("CR_RECOVERY_TYPE_RESTART"), recovery(renderer));
            assertEquals(Boolean.TRUE, renderer.getHostHdrReport());
        });
    }

    @Test
    public void anHdrStreamStillRestartsOnItsFirstSdrReport() {
        withRenderer(false, renderer -> {
            renderer.setHdrMode(false, null);
            assertEquals(constant("CR_RECOVERY_TYPE_RESTART"), recovery(renderer));
        });
    }

    @Test
    public void theInitialFormatOfATenBitSdrStreamIsSdrWithoutHdrKeys() {
        withRenderer(true, renderer -> {
            MediaFormat format = baseFormat(renderer);
            assertEquals(MediaFormat.COLOR_TRANSFER_SDR_VIDEO, format.getInteger(MediaFormat.KEY_COLOR_TRANSFER));
            assertEquals(MediaFormat.COLOR_STANDARD_BT709, format.getInteger(MediaFormat.KEY_COLOR_STANDARD));
            assertFalse(format.containsKey(MediaFormat.KEY_HDR_STATIC_INFO));
        });
    }

    @Test
    public void theInitialFormatOfAnHdrStreamLeavesTheColourKeysToTheDecoder() {
        withRenderer(false, renderer -> {
            MediaFormat format = baseFormat(renderer);
            assertFalse(format.containsKey(MediaFormat.KEY_COLOR_TRANSFER));
            assertFalse(format.containsKey(MediaFormat.KEY_COLOR_STANDARD));
        });
    }

    @Test
    public void theConfiguredFormatOfATenBitSdrStreamCarriesSdrKeysAndNoMasteringData() {
        MediaFormat format = MediaFormat.createVideoFormat("video/hevc", 1920, 1080);
        // A 10-bit SDR stream takes HDR to be off until the PC reports it on.
        boolean hdr = Sdr10Policy.assumeHdr(true, true);
        MediaCodecDecoderRenderer.applyConfigureColor(format, hdr, MoonBridge.COLORSPACE_REC_709, METADATA);
        assertEquals(MediaFormat.COLOR_TRANSFER_SDR_VIDEO, format.getInteger(MediaFormat.KEY_COLOR_TRANSFER));
        assertEquals(MediaFormat.COLOR_STANDARD_BT709, format.getInteger(MediaFormat.KEY_COLOR_STANDARD));
        assertFalse(format.containsKey(MediaFormat.KEY_HDR_STATIC_INFO));
    }

    @Test
    public void theConfiguredFormatOfAnHdrStreamKeepsItsHdrKeys() {
        MediaFormat format = MediaFormat.createVideoFormat("video/hevc", 1920, 1080);
        MediaCodecDecoderRenderer.applyConfigureColor(format, true, MoonBridge.COLORSPACE_REC_709, METADATA);
        assertEquals(MediaFormat.COLOR_TRANSFER_ST2084, format.getInteger(MediaFormat.KEY_COLOR_TRANSFER));
        assertEquals(MediaFormat.COLOR_STANDARD_BT2020, format.getInteger(MediaFormat.KEY_COLOR_STANDARD));
        assertTrue(format.containsKey(MediaFormat.KEY_HDR_STATIC_INFO));
    }
}

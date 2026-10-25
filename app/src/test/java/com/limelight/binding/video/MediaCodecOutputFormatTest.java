package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.media.MediaCodec;
import android.media.MediaFormat;

import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.shadows.ShadowMediaCodec;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, shadows = MediaCodecOutputFormatTest.FormatChangingCodec.class)
public class MediaCodecOutputFormatTest {
    @Test
    public void packedChromaSeparatesDisplaySizeFromCodedPadding() {
        Packed444 geometry = new Packed444(1, 2559, 1439, 2, 0);
        assertEquals(2559, geometry.width);
        assertEquals(1439, geometry.height);
        assertEquals(2560, geometry.paddedWidth);
        assertEquals(2880, geometry.paddedHeight * 2);
        assertEquals(2, geometry.colorSpace);
        assertEquals(0, geometry.colorRange);
    }

    @Test
    @Config(sdk = {23, 28, 29})
    public void packedSamplesBypassDecoderColorConversion() {
        MediaFormat format = MediaFormat.createVideoFormat("video/hevc", 2560, 2880);
        format.setInteger(MediaFormat.KEY_COLOR_STANDARD, MediaFormat.COLOR_STANDARD_BT2020);
        format.setInteger(MediaFormat.KEY_COLOR_TRANSFER, MediaFormat.COLOR_TRANSFER_ST2084);
        format.setInteger(MediaFormat.KEY_COLOR_RANGE, MediaFormat.COLOR_RANGE_LIMITED);
        format.setByteBuffer(MediaFormat.KEY_HDR_STATIC_INFO, java.nio.ByteBuffer.allocate(25));
        Packed444.sampleDataFormat(format);
        assertEquals(MediaFormat.COLOR_STANDARD_BT709, format.getInteger(MediaFormat.KEY_COLOR_STANDARD));
        assertEquals(MediaFormat.COLOR_TRANSFER_SDR_VIDEO, format.getInteger(MediaFormat.KEY_COLOR_TRANSFER));
        assertEquals(MediaFormat.COLOR_RANGE_FULL, format.getInteger(MediaFormat.KEY_COLOR_RANGE));
        assertNull(format.getByteBuffer(MediaFormat.KEY_HDR_STATIC_INFO));
    }

    @Test
    public void packedChromaRequiresOptInAndTheRustEngine() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
            MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity.get(), prefs,
                    null, 0, false, false, "", null);
            assertEquals(-1, renderer.setChromaPacking(1, 2560, 1440, 1, 0));
            prefs.experimentalPacked444 = true;
            assertEquals(-1, renderer.setChromaPacking(1, 2560, 1440, 1, 0));
            prefs.rustEngine = true;
            assertEquals(-1, renderer.setChromaPacking(3, 2560, 1440, 1, 0));
            assertEquals(0, renderer.setChromaPacking(2, 2560, 1440, 1, 0));
            assertEquals(2, ((Packed444) ReflectionHelpers.getField(renderer, "packing")).version);
            assertEquals(0, renderer.setChromaPacking(1, 2560, 1440, 1, 0));
            assertEquals(0, renderer.setChromaPacking(0, 2560, 1440, 1, 0));
            assertNull(ReflectionHelpers.getField(renderer, "packing"));
        }
    }

    @Test
    public void packedHdrIsLeftOutBeforeTheLaunchWhenNoTenBitModeVerifies() {
        int hdrFormats = com.limelight.nvstream.jni.MoonBridge.VIDEO_FORMAT_H265 |
                com.limelight.nvstream.jni.MoonBridge.VIDEO_FORMAT_H265_MAIN10;
        for (int modes : new int[] {0b0001, 0b0011, 0}) {
            try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
                ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
                PreferenceConfiguration prefs = new PreferenceConfiguration();
                prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
                prefs.experimentalPacked444 = true;
                prefs.rustEngine = true;
                final int probed = modes;
                MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity.get(), prefs,
                        null, 0, false, true, "", null) {
                    @Override public int preparePacked444Modes(int formats, int width, int height, int fps) {
                        return probed;
                    }
                };
                int gated = ReflectionHelpers.callInstanceMethod(renderer, "gatePackedHdr",
                        ReflectionHelpers.ClassParameter.from(int.class, hdrFormats),
                        ReflectionHelpers.ClassParameter.from(int.class, 1920),
                        ReflectionHelpers.ClassParameter.from(int.class, 1080),
                        ReflectionHelpers.ClassParameter.from(int.class, 120));
                if (modes == 0b0001) {
                    // 8-bit works, 10-bit does not: HDR is dropped, with a reason the connection shows.
                    assertEquals(com.limelight.nvstream.jni.MoonBridge.VIDEO_FORMAT_H265, gated);
                    assertNotNull(renderer.packedHdrDropReason());
                } else {
                    // 10-bit verified keeps HDR; no mode at all is preparePacked444Modes' own refusal.
                    assertEquals(hdrFormats, gated);
                    assertNull(renderer.packedHdrDropReason());
                }
            }
        }
    }

    private static MediaCodecDecoderRenderer packedRenderer(Activity activity, PreferenceConfiguration prefs,
                                                            java.util.function.IntUnaryOperator probe) {
        return new MediaCodecDecoderRenderer(activity, prefs, null, 0, false, true, "", null) {
            @Override int probePacked444Modes(int formats, int width, int height, int fps) {
                return probe.applyAsInt(fps);
            }
        };
    }

    @Test
    public void packedProbeRunsOncePerGeometryAndNeverWhenTheSessionHasPackedOff() {
        int hevc = com.limelight.nvstream.jni.MoonBridge.VIDEO_FORMAT_H265;
        int hevc10 = hevc | com.limelight.nvstream.jni.MoonBridge.VIDEO_FORMAT_H265_MAIN10;
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
            prefs.rustEngine = true;
            java.util.concurrent.atomic.AtomicInteger probes = new java.util.concurrent.atomic.AtomicInteger();
            MediaCodecDecoderRenderer renderer = packedRenderer(activity.get(), prefs, fps -> {
                probes.incrementAndGet();
                return 0b0011;
            });
            // A session reconnected without packed chroma: no probe, no modes, the PC is not asked for them.
            prefs.experimentalPacked444 = false;
            assertEquals(0, renderer.preparePacked444Modes(hevc10, 1920, 1080, 120));
            assertEquals(0, probes.get());
            assertEquals(hevc10, renderer.prepareVideoFormats(hevc10, 1920, 1080, 120));
            assertNull(renderer.packedHdrDropReason());
            assertEquals(0, probes.get());

            prefs.experimentalPacked444 = true;
            // The launch asks, then the connection: one decode, not two.
            assertEquals(0b0011, renderer.preparePacked444Modes(hevc10, 1920, 1080, 120));
            assertEquals(0b0011, renderer.preparePacked444Modes(hevc10, 1920, 1080, 120));
            assertEquals(0b0011, renderer.preparePacked444Modes(hevc, 1920, 1080, 120));
            assertEquals(1, probes.get());
            // Another size, rate or a format not probed before is a new question.
            renderer.preparePacked444Modes(hevc10, 1920, 1080, 60);
            renderer.preparePacked444Modes(hevc10, 2560, 1440, 60);
            assertEquals(3, probes.get());
            renderer.preparePacked444Modes(hevc10 | com.limelight.nvstream.jni.MoonBridge.VIDEO_FORMAT_AV1_MAIN8,
                    2560, 1440, 60);
            assertEquals(4, probes.get());
        }
    }

    @Test
    public void packedHdrDropNamesWhyTheTenBitPathFailedWithoutOverridingThePreparation() {
        int hdrFormats = com.limelight.nvstream.jni.MoonBridge.VIDEO_FORMAT_H265 |
                com.limelight.nvstream.jni.MoonBridge.VIDEO_FORMAT_H265_MAIN10;
        for (String issue : new String[] {MediaCodecDecoderRenderer.ISSUE_NO_HDR10_DISPLAY,
                MediaCodecDecoderRenderer.ISSUE_NO_TEN_BIT_OUTPUT, MediaCodecDecoderRenderer.ISSUE_TEN_BIT_SAMPLING, null}) {
            try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
                ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
                PreferenceConfiguration prefs = new PreferenceConfiguration();
                prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
                prefs.experimentalPacked444 = true;
                prefs.rustEngine = true;
                MediaCodecDecoderRenderer[] holder = new MediaCodecDecoderRenderer[1];
                holder[0] = packedRenderer(activity.get(), prefs, fps -> {
                    // What the real probe leaves behind: 8-bit verified, 10-bit not, and why.
                    ReflectionHelpers.setField(holder[0], "packedTenBitIssue", issue);
                    return 0b0001;
                });
                int launch = holder[0].prepareVideoFormats(hdrFormats, 1920, 1080, 120);
                assertEquals(com.limelight.nvstream.jni.MoonBridge.VIDEO_FORMAT_H265, launch);
                if (issue != null) assertEquals(issue, holder[0].packedHdrDropReason());
                else assertNotNull("a generic reason when the probe left none", holder[0].packedHdrDropReason());
                assertEquals(false, ReflectionHelpers.getField(holder[0], "packedTenBitOffered"));
            }
        }
    }

    @Test
    public void aPcWithoutTenBitPackedModesIsNamedAsTheReasonHdrIsOff() {
        int hevc = com.limelight.nvstream.jni.MoonBridge.VIDEO_FORMAT_H265;
        int hevc10 = com.limelight.nvstream.jni.MoonBridge.VIDEO_FORMAT_H265_MAIN10;
        String host = MediaCodecDecoderRenderer.ISSUE_HOST_NO_TEN_BIT;
        assertEquals("host doesn't offer 10-bit full chroma", host);
        // 10-bit offered, the handshake picked 8-bit packed: the PC had no 10-bit mode.
        assertEquals(host, MediaCodecDecoderRenderer.hostTenBitDrop(true, null, hevc));
        assertNull(MediaCodecDecoderRenderer.hostTenBitDrop(true, null, hevc | hevc10));
        // Never offered, or already left out at the launch for this device's own reason.
        assertNull(MediaCodecDecoderRenderer.hostTenBitDrop(false, null, hevc));
        assertEquals(MediaCodecDecoderRenderer.ISSUE_NO_HDR10_DISPLAY, MediaCodecDecoderRenderer.hostTenBitDrop(
                true, MediaCodecDecoderRenderer.ISSUE_NO_HDR10_DISPLAY, hevc));

        // The preparation marks a verified 10-bit mode as offered.
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
            prefs.experimentalPacked444 = true;
            prefs.rustEngine = true;
            MediaCodecDecoderRenderer renderer = packedRenderer(activity.get(), prefs, fps -> 0b0011);
            renderer.prepareVideoFormats(hevc | hevc10, 1920, 1080, 60);
            assertNull(renderer.packedHdrDropReason());
            assertEquals(true, ReflectionHelpers.getField(renderer, "packedTenBitOffered"));
            // A session without packed chroma offers none.
            prefs.experimentalPacked444 = false;
            renderer.prepareVideoFormats(hevc | hevc10, 1920, 1080, 60);
            assertEquals(false, ReflectionHelpers.getField(renderer, "packedTenBitOffered"));
        }
    }

    @Test
    public void tenBitOutputIssueNamesTheDisplayBeforeTheGpu() {
        assertNull(MediaCodecDecoderRenderer.tenBitOutputIssue(true, true));
        assertEquals(MediaCodecDecoderRenderer.ISSUE_NO_HDR10_DISPLAY, MediaCodecDecoderRenderer.tenBitOutputIssue(false, true));
        assertEquals(MediaCodecDecoderRenderer.ISSUE_NO_HDR10_DISPLAY, MediaCodecDecoderRenderer.tenBitOutputIssue(false, false));
        assertEquals(MediaCodecDecoderRenderer.ISSUE_NO_TEN_BIT_OUTPUT, MediaCodecDecoderRenderer.tenBitOutputIssue(true, false));
    }

    @Test
    public void drainingQueuedFramesStillProcessesFormatChanges() throws Exception {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
            prefs.framePacing = PreferenceConfiguration.FRAME_PACING_MIN_LATENCY;
            MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity.get(), prefs,
                    null, 0, false, false, "", null);
            renderer.setStreamFrameRateMilliHz(60_000);
            FormatChangingCodec.renderer = renderer;
            FormatChangingCodec.format = MediaFormat.createVideoFormat("video/avc", 1920, 1080);
            MediaCodec codec = MediaCodec.createDecoderByType("video/avc");
            try {
                ReflectionHelpers.setField(renderer, "videoDecoder", codec);
                ReflectionHelpers.callInstanceMethod(renderer, "startRendererThread");
                Thread thread = ReflectionHelpers.getField(renderer, "rendererThread");
                try {
                    thread.join(5000);
                    assertFalse("Renderer did not release the queued frame", thread.isAlive());
                    assertSame(FormatChangingCodec.format, ReflectionHelpers.getField(renderer, "outputFormat"));
                } finally {
                    renderer.stop();
                }
            } finally {
                codec.release();
            }
        }
    }

    @Implements(MediaCodec.class)
    public static class FormatChangingCodec extends ShadowMediaCodec {
        static MediaCodecDecoderRenderer renderer;
        static MediaFormat format;
        private int dequeues;

        @Implementation
        public int dequeueOutputBuffer(MediaCodec.BufferInfo info, long timeoutUs) {
            info.presentationTimeUs = 1;
            return dequeues++ == 0 ? 0 : MediaCodec.INFO_OUTPUT_FORMAT_CHANGED;
        }

        @Implementation
        public MediaFormat getOutputFormat() {
            return format;
        }

        @Implementation
        public void releaseOutputBuffer(int index, long timestampNs) {
            ReflectionHelpers.setField(renderer, "stopping", true);
        }
    }
}

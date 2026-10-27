package com.limelight.binding.video;

import android.app.Application;
import android.media.MediaCodecInfo;
import android.media.MediaCodecInfo.CodecCapabilities;
import android.media.MediaCodecInfo.CodecProfileLevel;
import android.media.MediaFormat;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.MediaCodecInfoBuilder;
import org.robolectric.shadows.MediaCodecInfoBuilder.CodecCapabilitiesBuilder;
import org.robolectric.util.ReflectionHelpers;

import java.util.Arrays;
import java.util.List;
import java.util.Map;

import static org.junit.Assert.*;

/**
 * Which decoder tuning keys each decoder name gets. The vendor sets follow Artemis (ClassicOldSong's
 * Moonlight fork, GPL-3.0) and the derflacco fork moonlight-noir (GPL-3.0); every key goes only to
 * the decoders it belongs to.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class DecoderTuningKeysTest {
    private static final String HEVC = "video/hevc";
    private static final String[] QTI_KEYS = {
            "vendor.qti-ext-dec-low-latency.enable", "vendor.qti-ext-dec-picture-order.enable",
            "vendor.qti-ext-output-sw-fence-enable.value", "vendor.qti-ext-output-fence.enable",
            "vendor.qti-ext-output-fence.fence_type"};
    private static final String[] TEGRA_KEYS = {
            "media.low-latency.enable", "vendor.low-latency.enable", "disable-output-reorder",
            "vendor.nvidia.disable-output-reorder"};
    private static final String[] MTK_KEYS = {
            "vendor.mtk.vdec.low-latency.mode", "vendor.mtk.vdec.cpu.boost.mode", "vendor.mtk.vdec.cpu.boost.mode.value",
            "vendor.mtk.vdec.dvfs.mode", "vendor.mtk.vdec.dvfs.level", "vendor.mtk.vdec.disable-idle",
            "vendor.mtk.vdec.vsync.adjust.enable", "vendor.mtk.vdec.preload.frame.count", "vendor.mtk.vdec.input.max.queue.depth",
            "vendor.mtk.vdec.output.max.queue.depth", "vendor.mtk.vdec.bq.guard.interval.time", "vendor.mtk.vdec.pq",
            "vendor.mtk.vdec.skip.mode", "vendor.mtk.vdec.drop.nonref.frame"};

    @Before
    public void setUp() {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
    }

    @Test
    public void snapdragonDecodersGetOnlyTheQualcommKeys() {
        for (String name : new String[] {"c2.qti.hevc.decoder.low_latency", "c2.qti.hevc.decoder", "c2.qcom.hevc.decoder",
                "OMX.qcom.video.decoder.hevc"}) {
            MediaFormat format = tune(name, 0, true);
            assertKeys(name, format, QTI_KEYS);
            assertEquals(name, Short.MAX_VALUE, format.getInteger(MediaFormat.KEY_OPERATING_RATE));
            assertFalse(name, format.containsKey(MediaFormat.KEY_PRIORITY));
            assertAbsent(name, format, TEGRA_KEYS);
            assertAbsent(name, format, MTK_KEYS);
            assertAbsent(name, format, "vendor.rtc-ext-dec-low-latency.enable", "vendor.low-latency.enable");
            // The OMX layer reads vdec-lowlatency only on OMX decoders that aren't Qualcomm's.
            assertFalse(name, format.containsKey("vdec-lowlatency"));
        }
    }

    @Test
    public void tegraDecodersGetTheNvidiaKeysByName() {
        for (String name : new String[] {"OMX.Nvidia.h265.decode", "c2.nvidia.hevc.decoder"}) {
            MediaFormat format = tune(name, 0, false);
            assertKeys(name, format, TEGRA_KEYS);
            assertAbsent(name, format, QTI_KEYS);
            assertAbsent(name, format, MTK_KEYS);
            assertEquals(name, name.startsWith("OMX"), format.containsKey("vdec-lowlatency"));
        }
    }

    @Test
    public void tegraKeysFollowTheFallbackLadderAndTheVendorSwitch() {
        for (int attempt = 0; attempt <= 5; attempt++) {
            MediaFormat format = tune("OMX.Nvidia.h265.decode", attempt, false);
            for (String key : TEGRA_KEYS) {
                assertEquals("try " + attempt + " " + key, attempt < 4, format.containsKey(key));
            }
        }
        MediaFormat off = MediaFormat.createVideoFormat(HEVC, 1920, 1080);
        MediaCodecHelper.setDecoderLowLatencyOptions(off, decoder("OMX.Nvidia.h265.decode"), 0, true, false, false, false);
        assertAbsent("vendor switch off", off, TEGRA_KEYS);
    }

    @Test
    public void mediaTekDecodersGetTheWholePresetByName() {
        for (String name : new String[] {"c2.mtk.hevc.decoder", "OMX.MTK.VIDEO.DECODER.HEVC"}) {
            MediaFormat format = tune(name, 0, false);
            assertKeys(name, format, MTK_KEYS);
            assertEquals(name, 0, format.getInteger("vendor.mtk.vdec.vsync.adjust.enable"));
            assertEquals(name, 2, format.getInteger("vendor.mtk.vdec.dvfs.level"));
            assertEquals(name, 4, format.getInteger("vendor.mtk.vdec.buffer.fetch.timeout.ms"));
            assertEquals(name, 0, format.getInteger("vendor.mtk.vdec.pq"));
            // The rate and priority pair sits behind the performance hints.
            assertAbsent(name, format, MediaFormat.KEY_OPERATING_RATE, MediaFormat.KEY_PRIORITY);
            MediaFormat hinted = tune(name, 0, true);
            assertEquals(name, Short.MAX_VALUE, hinted.getInteger(MediaFormat.KEY_OPERATING_RATE));
            // The real-time priority next to the top rate is what crashes decoders after start().
            assertFalse(name, hinted.containsKey(MediaFormat.KEY_PRIORITY));
            assertEquals(name, name.startsWith("OMX"), format.containsKey("vdec-lowlatency"));
            assertAbsent(name, format, QTI_KEYS);
            assertAbsent(name, format, TEGRA_KEYS);
            assertAbsent(name, format, "vendor.mtk.vdec.ultra-low-latency");
        }
    }

    @Test
    public void mediaTekPresetDropsOutOnTheFallbackTries() {
        for (int attempt = 0; attempt <= 4; attempt++) {
            MediaFormat format = tune("c2.mtk.hevc.decoder", attempt, true);
            for (String key : MTK_KEYS) {
                assertEquals("try " + attempt + " " + key, attempt < 2, format.containsKey(key));
            }
            assertEquals("try " + attempt, attempt < 3, format.containsKey(MediaFormat.KEY_OPERATING_RATE));
        }
        assertAbsent("vendor switch off", tuneWithoutVendorKeys("c2.mtk.hevc.decoder"), MTK_KEYS);
    }

    @Test
    @Config(sdk = 24)
    public void mediaTekPresetNeedsVendorKeySupport() {
        // Fire OS 5 OMX.MTK builds on Android 5.1 and 7 have no vendor format keys.
        assertAbsent("Android 7", tune("OMX.MTK.VIDEO.DECODER.HEVC", 0, false), MTK_KEYS);
    }

    @Test
    @Config(sdk = 34)
    public void androidTwelveSendsTheMediaTekPresetEvenWhenTheDecoderListsLess() {
        MediaCodecInfo decoder = decoder("c2.mtk.hevc.decoder", "vendor.mtk.vdec.low-latency.mode");
        MediaFormat format = MediaFormat.createVideoFormat(HEVC, 1920, 1080);
        assertTrue(MediaCodecHelper.setDecoderLowLatencyOptions(format, decoder, 0, true, true, false, false));
        assertKeys("preset", format, MTK_KEYS);
        assertKeys("preset", format, "vendor.mtk.vdec.nvop.skip", "vendor.mtk.vdec.frame-drop.policy");
    }

    @Test
    public void snapdragonDecodersGetTheForkExtrasOnTheFirstTriesOnly() {
        for (String name : new String[] {"c2.qti.hevc.decoder.low_latency", "OMX.qcom.video.decoder.hevc"}) {
            MediaFormat first = tune(name, 0, false);
            // The fork's picture-order 0 left the Snapdragon 8 Gen 2 decoder without output.
            assertEquals(name, 1, first.getInteger("vendor.qti-ext-dec-picture-order.enable"));
            assertEquals(name, 1, first.getInteger("vendor.qti-ext-dec-frame-drop.enable"));
            assertEquals(name, name.startsWith("OMX"), first.containsKey("vendor.qti-ext-dec-dpb-output-delay.enable"));
            assertEquals(name, name.startsWith("OMX"), first.containsKey("vendor.qti-ext-dec-picture-type.enable"));
            MediaFormat third = tune(name, 3, false);
            assertAbsent(name, third, "vendor.qti-ext-dec-frame-drop.enable", "vendor.qti-ext-dec-dpb-output-delay.enable",
                    "vendor.qti-ext-dec-picture-type.enable");
        }
        assertAbsent("mtk", tune("c2.mtk.hevc.decoder", 0, false), "vendor.qti-ext-dec-frame-drop.enable");
        assertAbsent("exynos", tune("c2.exynos.hevc.decoder", 0, false), "vendor.qti-ext-dec-frame-drop.enable");
    }

    @Test
    public void tegraDecodersGetTheTopRateOnlyBehindThePerformanceHints() {
        assertAbsent("hints off", tune("OMX.Nvidia.h265.decode", 0, false), MediaFormat.KEY_OPERATING_RATE, MediaFormat.KEY_PRIORITY);
        MediaFormat format = tune("OMX.Nvidia.h265.decode", 0, true);
        assertEquals(Short.MAX_VALUE, format.getInteger(MediaFormat.KEY_OPERATING_RATE));
        assertFalse(format.containsKey(MediaFormat.KEY_PRIORITY));
        // With the vendor switch off, the generic non-Qualcomm priority hint is all it gets.
        MediaFormat off = MediaFormat.createVideoFormat(HEVC, 1920, 1080);
        MediaCodecHelper.setDecoderLowLatencyOptions(off, decoder("OMX.Nvidia.h265.decode"), 0, true, false, true, false);
        assertFalse(off.containsKey(MediaFormat.KEY_OPERATING_RATE));
        assertEquals(0, off.getInteger(MediaFormat.KEY_PRIORITY));
    }

    @Test
    public void referenceFrameInvalidationCoversMediaTekAndCodec2Snapdragon() {
        MediaCodecHelper.addMediaTekAndQcomRefFrameInvalidation();
        for (String name : new String[] {"c2.mtk.avc.decoder", "OMX.MTK.VIDEO.DECODER.AVC"}) {
            assertTrue(name, MediaCodecHelper.decoderSupportsRefFrameInvalidationAvc(name, 1080));
        }
        for (String name : new String[] {"c2.mtk.hevc.decoder", "OMX.MTK.VIDEO.DECODER.HEVC", "c2.qcom.hevc.decoder"}) {
            assertTrue(name, MediaCodecHelper.decoderSupportsRefFrameInvalidationHevc(decoder(name), false, false, false));
        }
        assertFalse(MediaCodecHelper.decoderSupportsRefFrameInvalidationAvc("c2.acme.avc.decoder", 1080));
        assertFalse(MediaCodecHelper.decoderSupportsRefFrameInvalidationHevc(decoder("c2.acme.hevc.decoder"), false, false, false));
    }

    @Test
    @Config(sdk = 34)
    public void aMediaTekSwitchFromTheForkMarksAReferenceInvalidationDecoder() {
        MediaCodecInfo decoder = decoder("c2.acme.hevc.decoder", "vendor.mtk.vdec.early.output.enable");
        assertTrue(MediaCodecHelper.decoderSupportsRefFrameInvalidationHevc(decoder, false, true, false));
        assertFalse(MediaCodecHelper.decoderSupportsRefFrameInvalidationHevc(decoder, false, false, false));
    }

    @Test
    @Config(sdk = 34)
    public void mediaTekDelayKnobsAreNoEvidenceAndAv1SelectionIsUnchanged() {
        for (String knob : new String[] {"vendor.mtk.vdec.buffer.fetch.timeout.ms", "vendor.mtk.vdec.output.delay.frames",
                "vendor.mtk.vdec.dpb.output.delay.frames"}) {
            MediaCodecInfo decoder = decoder("c2.acme.hevc.decoder", knob);
            assertFalse(knob, MediaCodecHelper.decoderSupportsRefFrameInvalidationHevc(decoder, false, true, false));
        }
        // The fork's RFI switches do not make a decoder pass the AV1 whitelist.
        MediaCodecInfo mtkSwitch = decoder("c2.acme.av1.decoder", "vendor.mtk.vdec.early.output.enable");
        assertFalse(MediaCodecHelper.isDecoderWhitelistedForAv1(mtkSwitch));
        // A known low-latency switch still does.
        assertTrue(MediaCodecHelper.isDecoderWhitelistedForAv1(
                decoder("c2.acme.av1.decoder", "vendor.low-latency.enable")));
    }

    @Test
    @Config(sdk = 33)
    public void theRetroidDecoderGetsExactlyThisKeySet() {
        MediaCodecInfo decoder = decoder("c2.qti.hevc.decoder.low_latency", QTI_KEYS);
        MediaFormat format = MediaFormat.createVideoFormat(HEVC, 1920, 1080);
        assertTrue(MediaCodecHelper.setDecoderLowLatencyOptions(format, decoder, 0, true, true, true, false));
        java.util.Set<String> expected = new java.util.HashSet<>(Arrays.asList(QTI_KEYS));
        expected.addAll(Arrays.asList(MediaFormat.KEY_MIME, MediaFormat.KEY_WIDTH, MediaFormat.KEY_HEIGHT,
                MediaFormat.KEY_LOW_LATENCY, MediaFormat.KEY_OPERATING_RATE, "vendor.qti-ext-dec-frame-drop.enable"));
        assertEquals(expected, format.getKeys());
    }

    private static final String[] QTI_EXTRAS = {"vendor.qti-ext-dec-info-misr.disable",
            "vendor.qti-ext-dec-instant-decode.enable", "vendor.qti-ext-dec-error-correction.conceal"};

    @Test
    @Config(sdk = 33)
    public void snapdragonExtrasFollowTheDeclaredKeysAndTheLadder() {
        String[] declared = Arrays.copyOf(QTI_KEYS, QTI_KEYS.length + QTI_EXTRAS.length);
        System.arraycopy(QTI_EXTRAS, 0, declared, QTI_KEYS.length, QTI_EXTRAS.length);
        MediaCodecInfo decoder = decoder("c2.qti.hevc.decoder", declared);
        for (int attempt = 0; attempt <= 5; attempt++) {
            MediaFormat format = MediaFormat.createVideoFormat(HEVC, 1920, 1080);
            MediaCodecHelper.setDecoderLowLatencyOptions(format, decoder, attempt, true, true, false, false);
            for (String key : QTI_EXTRAS) {
                assertEquals("try " + attempt + " " + key, attempt < 4, format.containsKey(key));
            }
            // Never the picture-order value that hung the Retroid.
            if (format.containsKey("vendor.qti-ext-dec-picture-order.enable")) {
                assertEquals(1, format.getInteger("vendor.qti-ext-dec-picture-order.enable"));
            }
        }
        MediaFormat off = MediaFormat.createVideoFormat(HEVC, 1920, 1080);
        MediaCodecHelper.setDecoderLowLatencyOptions(off, decoder, 0, true, false, false, false);
        assertAbsent("vendor switch off", off, QTI_EXTRAS);
    }

    @Test
    public void snapdragonExtrasAreNeverSentByName() {
        // Android 9: the decoder cannot list its parameters, so only the name-matched set goes.
        for (String name : new String[] {"c2.qti.hevc.decoder", "OMX.qcom.video.decoder.hevc"}) {
            MediaFormat format = MediaFormat.createVideoFormat(HEVC, 1920, 1080);
            MediaCodecHelper.setDecoderLowLatencyOptions(format, decoder(name, QTI_EXTRAS), 0, true, true, true, false);
            assertKeys(name, format, "vendor.qti-ext-dec-low-latency.enable");
            assertAbsent(name, format, QTI_EXTRAS);
        }
    }

    @Test
    @Config(sdk = 33)
    public void snapdragonExtrasStayOffDecodersThatDoNotDeclareThem() {
        MediaFormat format = MediaFormat.createVideoFormat(HEVC, 1920, 1080);
        MediaCodecHelper.setDecoderLowLatencyOptions(format, decoder("c2.qti.hevc.decoder.low_latency", QTI_KEYS),
                0, true, true, true, false);
        assertAbsent("undeclared", format, QTI_EXTRAS);
    }

    @Test
    public void otherChipsKeepTheirOwnKeysAndNothingFromTheNewSets() {
        String[][] cases = {
                {"c2.exynos.hevc.decoder", "vendor.rtc-ext-dec-low-latency.enable"},
                {"OMX.hisi.video.decoder.hevc", "vendor.hisi-ext-low-latency-video-dec.video-scene-for-low-latency-req"},
                {"c2.amlogic.hevc.decoder", "vendor.low-latency.enable"},
        };
        for (String[] c : cases) {
            MediaFormat format = tune(c[0], 0, false);
            assertEquals(c[0], 1, format.getInteger(c[1]));
            assertAbsent(c[0], format, QTI_KEYS);
            assertAbsent(c[0], format, MTK_KEYS);
            assertAbsent(c[0], format, "media.low-latency.enable", "disable-output-reorder", "vendor.nvidia.disable-output-reorder");
        }
        assertFalse(tune("c2.exynos.hevc.decoder", 0, false).containsKey("vendor.low-latency.enable"));
    }

    private static final String[] EXYNOS_KEYS = {
            "vendor.rtc-ext-dec-low-latency.enable", "vendor.rtc-ext-dec-output-queue-depth.value", "vendor.sec-dec-output.delay"};

    @Test
    public void exynosDecodersGetTheWholePresetForEveryCodec() {
        for (String name : new String[] {"c2.exynos.h264.decoder", "c2.exynos.hevc.decoder", "c2.exynos.av1.decoder",
                "OMX.Exynos.avc.dec", "OMX.Exynos.HEVC.Decoder"}) {
            MediaFormat format = tune(name, 0, false);
            assertKeys(name, format, EXYNOS_KEYS);
            assertEquals(name, 1, format.getInteger("vendor.rtc-ext-dec-low-latency.enable"));
            assertEquals(name, 2, format.getInteger("vendor.rtc-ext-dec-output-queue-depth.value"));
            assertEquals(name, 0, format.getInteger("vendor.sec-dec-output.delay"));
            assertAbsent(name, format, QTI_KEYS);
            assertAbsent(name, format, TEGRA_KEYS);
            assertAbsent(name, format, MTK_KEYS);
            assertAbsent(name, format, MediaFormat.KEY_OPERATING_RATE, VendorLowLatencyKeys.GOOGLE_FRAME_RATE);
        }
    }

    @Test
    @Config(sdk = 34)
    public void androidTwelveSendsTheExynosPresetEvenWhenTheDecoderListsLessOrNothing() {
        // The Galaxy A37 lists the switch for H.264 only (issue 26); HEVC lists no vendor key.
        MediaCodecInfo avc = decoder("c2.exynos.h264.decoder", "vendor.rtc-ext-dec-low-latency.enable");
        MediaCodecInfo hevc = decoder("c2.exynos.hevc.decoder", "vendor.acme.unrelated.value");
        for (MediaCodecInfo info : new MediaCodecInfo[] {avc, hevc}) {
            MediaFormat format = MediaFormat.createVideoFormat(HEVC, 1920, 1080);
            assertTrue(MediaCodecHelper.setDecoderLowLatencyOptions(format, info, 0, true, true, false, false));
            assertKeys(info.getName(), format, EXYNOS_KEYS);
        }
    }

    @Test
    public void exynosPresetFollowsTheFallbackLadderAndTheVendorSwitch() {
        for (int attempt = 0; attempt <= 5; attempt++) {
            MediaFormat format = tune("c2.exynos.hevc.decoder", attempt, false);
            // The queue and delay keys leave after the second try; the switch stays through the fourth.
            assertEquals("try " + attempt, attempt < 4, format.containsKey("vendor.rtc-ext-dec-low-latency.enable"));
            assertEquals("try " + attempt, attempt < 2, format.containsKey("vendor.rtc-ext-dec-output-queue-depth.value"));
            assertEquals("try " + attempt, attempt < 2, format.containsKey("vendor.sec-dec-output.delay"));
        }
        assertAbsent("vendor switch off", tuneWithoutVendorKeys("c2.exynos.hevc.decoder"), EXYNOS_KEYS);
        MediaFormat blocked = MediaFormat.createVideoFormat(HEVC, 1920, 1080);
        MediaCodecHelper.setDecoderLowLatencyOptions(blocked, decoder("c2.exynos.hevc.decoder"), 0, true, true, false, true);
        assertAbsent("HEVC override", blocked, EXYNOS_KEYS);
    }

    @Test
    public void exynosKeysAreNotSentToOtherChips() {
        for (String name : new String[] {"c2.qti.hevc.decoder", "OMX.qcom.video.decoder.hevc", "c2.mtk.hevc.decoder",
                "OMX.MTK.VIDEO.DECODER.HEVC", "OMX.Nvidia.h265.decode", "c2.google.hevc.decoder", "c2.android.hevc.decoder"}) {
            MediaFormat format = tune(name, 0, true);
            assertAbsent(name, format, "vendor.rtc-ext-dec-output-queue-depth.value", "vendor.sec-dec-output.delay");
            assertAbsent(name, format, "vendor.rtc-ext-dec-low-latency.enable");
        }
    }

    @Test
    public void tensorDecodersRunAtTheirMostAndTheStreamRateCountsAtLeast() {
        for (String name : new String[] {"c2.google.hevc.decoder", "c2.google.h264.decoder", "c2.google.av1.decoder"}) {
            MediaFormat format = tune(name, 0, true);
            assertEquals(name, 120, format.getInteger(MediaFormat.KEY_OPERATING_RATE));
            assertEquals(name, 0, format.getInteger(MediaFormat.KEY_PRIORITY));
            assertAbsent(name, format, EXYNOS_KEYS);
            assertAbsent(name, format, QTI_KEYS);
            assertAbsent(name, format, MTK_KEYS);
            // The frame rate key is Android 16 (API 36) and newer.
            assertFalse(name, format.containsKey(VendorLowLatencyKeys.GOOGLE_FRAME_RATE));
        }
        MediaFormat fast = MediaFormat.createVideoFormat(HEVC, 1920, 1080);
        fast.setInteger(MediaFormat.KEY_FRAME_RATE, 240);
        MediaCodecHelper.setDecoderLowLatencyOptions(fast, decoder("c2.google.hevc.decoder"), 0, true, true, true, false);
        assertEquals(240, fast.getInteger(MediaFormat.KEY_OPERATING_RATE));
    }

    @Test
    public void tensorKeysFollowTheFallbackLadderAndTheVendorSwitch() {
        for (int attempt = 0; attempt <= 3; attempt++) {
            MediaFormat format = tune("c2.google.hevc.decoder", attempt, true);
            assertEquals("try " + attempt, attempt < 2, format.containsKey(MediaFormat.KEY_OPERATING_RATE));
            // The performance hints give every non-Qualcomm decoder priority 0 through try 2.
            assertEquals("try " + attempt, attempt < 3, format.containsKey(MediaFormat.KEY_PRIORITY));
        }
        assertAbsent("vendor switch off", tuneWithoutVendorKeys("c2.google.hevc.decoder"),
                MediaFormat.KEY_OPERATING_RATE, MediaFormat.KEY_PRIORITY, VendorLowLatencyKeys.GOOGLE_FRAME_RATE);
    }

    @Test
    @Config(sdk = 35)
    public void tensorDecodersGetTheirFrameRateKeyOnAndroidSixteenOnTheFirstTryOnly() {
        // Robolectric has no API 36 yet, so the level is raised on the Build class itself.
        int before = android.os.Build.VERSION.SDK_INT;
        ReflectionHelpers.setStaticField(android.os.Build.VERSION.class, "SDK_INT", 36);
        try {
            MediaFormat first = tune("c2.google.hevc.decoder", 0, true);
            assertEquals(120, first.getInteger(VendorLowLatencyKeys.GOOGLE_FRAME_RATE));
            assertFalse(tune("c2.google.hevc.decoder", 1, true).containsKey(VendorLowLatencyKeys.GOOGLE_FRAME_RATE));
            assertFalse(tune("c2.exynos.hevc.decoder", 0, true).containsKey(VendorLowLatencyKeys.GOOGLE_FRAME_RATE));
        } finally {
            ReflectionHelpers.setStaticField(android.os.Build.VERSION.class, "SDK_INT", before);
        }
    }

    @Test
    public void tensorRateAndPriorityNeedThePerformanceHints() {
        MediaFormat format = tune("c2.google.hevc.decoder", 0, false);
        assertAbsent("hints off", format, MediaFormat.KEY_OPERATING_RATE, MediaFormat.KEY_PRIORITY,
                VendorLowLatencyKeys.GOOGLE_FRAME_RATE);
    }

    @Test
    public void tensorOperatingRateStaysWithinWhatTheDecoderDeclares() {
        // Below the 120 floor the decoder's own ceiling wins; the floor needs a decoder that reaches it.
        assertEquals(60, MediaCodecHelper.googleOperatingRate(60, 60.0));
        assertEquals(60, MediaCodecHelper.googleOperatingRate(30, 60.0));
        assertEquals(60, MediaCodecHelper.googleOperatingRate(90, 60.0));
        assertEquals(120, MediaCodecHelper.googleOperatingRate(60, 120.0));
        // Above it the decoder's maximum sets the rate, capped at 960.
        assertEquals(480, MediaCodecHelper.googleOperatingRate(60, 480.0));
        assertEquals(480, MediaCodecHelper.googleOperatingRate(240, 480.0));
        assertEquals(960, MediaCodecHelper.googleOperatingRate(60, 2000.0));
        // No capabilities for the size: the stream's rate, at least 120.
        assertEquals(120, MediaCodecHelper.googleOperatingRate(60, null));
        assertEquals(240, MediaCodecHelper.googleOperatingRate(240, null));
    }

    @Test
    public void chipsetTuningIsOptInAndLimitedToItsDecodersAndTheFirstTry() {
        // Off (the default): Codec2 MediaTek gets no vdec-lowlatency, Exynos only priority 0.
        assertFalse(tune("c2.mtk.hevc.decoder", 0, true).containsKey("vdec-lowlatency"));
        MediaFormat exynosDefault = tune("c2.exynos.hevc.decoder", 0, true);
        assertFalse(exynosDefault.containsKey(MediaFormat.KEY_OPERATING_RATE));
        assertEquals(0, exynosDefault.getInteger(MediaFormat.KEY_PRIORITY));

        assertEquals(1, tuneChipset("c2.mtk.hevc.decoder", 0, true).getInteger("vdec-lowlatency"));
        assertFalse(tuneChipset("c2.mtk.hevc.decoder", 1, true).containsKey("vdec-lowlatency"));
        MediaFormat exynos = tuneChipset("c2.exynos.hevc.decoder", 0, true);
        assertEquals(120, exynos.getInteger(MediaFormat.KEY_OPERATING_RATE));
        assertEquals(0, exynos.getInteger(MediaFormat.KEY_PRIORITY));
        // Later tries fall back to the default ladder.
        MediaFormat exynosRetry = tuneChipset("c2.exynos.hevc.decoder", 1, true);
        assertFalse(exynosRetry.containsKey(MediaFormat.KEY_OPERATING_RATE));
        assertEquals(0, exynosRetry.getInteger(MediaFormat.KEY_PRIORITY));
        // The Exynos rate needs the performance hints, like Tensor's.
        assertFalse(tuneChipset("c2.exynos.hevc.decoder", 0, false).containsKey(MediaFormat.KEY_OPERATING_RATE));

        for (String name : new String[] {"c2.qti.hevc.decoder", "c2.google.hevc.decoder", "c2.android.hevc.decoder",
                "c2.acme.hevc.decoder"}) {
            assertFalse(name, tuneChipset(name, 0, true).containsKey("vdec-lowlatency"));
        }
        assertFalse(tuneChipset("c2.qti.hevc.decoder", 0, true).containsKey(MediaFormat.KEY_PRIORITY));

        // The vendor switch and the HEVC override still win.
        MediaFormat vendorOff = MediaFormat.createVideoFormat(HEVC, 1920, 1080);
        MediaCodecHelper.setDecoderLowLatencyOptions(vendorOff, decoder("c2.mtk.hevc.decoder"), 0, true, false, true, false, true);
        assertFalse(vendorOff.containsKey("vdec-lowlatency"));
        MediaFormat hevcOff = MediaFormat.createVideoFormat(HEVC, 1920, 1080);
        MediaCodecHelper.setDecoderLowLatencyOptions(hevcOff, decoder("c2.exynos.hevc.decoder"), 0, true, true, true, true, true);
        assertFalse(hevcOff.containsKey(MediaFormat.KEY_OPERATING_RATE));
    }

    @Test
    public void unknownDecodersGetOnlyTheStandardKey() {
        MediaFormat format = tune("c2.acme.hevc.decoder", 0, false);
        assertEquals(1, format.getInteger(MediaFormat.KEY_LOW_LATENCY));
        assertAbsent("acme", format, QTI_KEYS);
        assertAbsent("acme", format, TEGRA_KEYS);
        assertAbsent("acme", format, MTK_KEYS);
        assertFalse(format.containsKey("vdec-lowlatency"));
    }

    @Test
    public void standardLowLatencyKeyIsSentBeforeAndroidEleven() {
        // Some vendor builds read it before Android 11; older frameworks ignore it.
        assertEquals(1, tune("c2.acme.hevc.decoder", 0, false).getInteger(MediaFormat.KEY_LOW_LATENCY));
        assertFalse(tune("c2.acme.hevc.decoder", 1, false).containsKey(MediaFormat.KEY_LOW_LATENCY));
    }

    @Test
    public void requestedAndDroppedKeysAreListedForTheReport() {
        MediaFormat first = tune("c2.mtk.hevc.decoder", 0, false);
        MediaFormat last = tune("c2.mtk.hevc.decoder", 4, false);
        List<String> firstKeys = MediaCodecHelper.requestedTuningKeys(first, null);
        List<String> lastKeys = MediaCodecHelper.requestedTuningKeys(last, null);
        assertTrue(firstKeys.contains("vendor.mtk.vdec.dvfs.mode=2"));
        assertTrue(firstKeys.contains("vendor.mtk.vdec.vsync.adjust.enable=0"));
        assertTrue(firstKeys.contains("low-latency=1"));
        assertTrue(lastKeys.isEmpty());
        List<String> dropped = MediaCodecHelper.droppedTuningKeys(firstKeys, lastKeys);
        assertTrue(dropped.contains("vendor.mtk.vdec.dvfs.mode=2"));
        assertTrue(MediaCodecHelper.droppedTuningKeys(firstKeys, firstKeys).isEmpty());
    }

    @Test
    public void hdrStaticInfoIsNotWrittenAsNullBeforeAndroidTen() {
        // Fire OS 7 (Android 9) OMX decoders may refuse a null entry; a fresh format must stay clean.
        MediaFormat fresh = MediaFormat.createVideoFormat(HEVC, 1920, 1080);
        MediaCodecDecoderRenderer.applyHdrStaticInfo(fresh, null);
        assertFalse(fresh.containsKey(MediaFormat.KEY_HDR_STATIC_INFO));
        java.nio.ByteBuffer info = java.nio.ByteBuffer.allocate(25);
        MediaCodecDecoderRenderer.applyHdrStaticInfo(fresh, info);
        assertSame(info, fresh.getByteBuffer(MediaFormat.KEY_HDR_STATIC_INFO));
    }

    private static MediaFormat tuneWithoutVendorKeys(String name) {
        MediaFormat format = MediaFormat.createVideoFormat(HEVC, 1920, 1080);
        MediaCodecHelper.setDecoderLowLatencyOptions(format, decoder(name), 0, true, false, false, false);
        return format;
    }

    private static MediaFormat tune(String name, int attempt, boolean performanceHints) {
        MediaFormat format = MediaFormat.createVideoFormat(HEVC, 1920, 1080);
        MediaCodecHelper.setDecoderLowLatencyOptions(format, decoder(name), attempt, true, true, performanceHints, false);
        return format;
    }

    private static MediaFormat tuneChipset(String name, int attempt, boolean performanceHints) {
        MediaFormat format = MediaFormat.createVideoFormat(HEVC, 1920, 1080);
        MediaCodecHelper.setDecoderLowLatencyOptions(format, decoder(name), attempt, true, true, performanceHints, false, true);
        return format;
    }

    private static void assertKeys(String label, MediaFormat format, String... keys) {
        for (String key : keys) {
            assertTrue(label + " should have " + key, format.containsKey(key));
        }
    }

    private static void assertAbsent(String label, MediaFormat format, String... keys) {
        for (String key : keys) {
            assertFalse(label + " should not have " + key, format.containsKey(key));
        }
    }

    @Test
    public void experimentalLabelCoversVendorsWeCannotRun() {
        assertTrue(MediaCodecHelper.isExperimentalVendorDecoder("c2.mtk.hevc.decoder"));
        assertTrue(MediaCodecHelper.isExperimentalVendorDecoder("OMX.MTK.VIDEO.DECODER.HEVC"));
        assertTrue(MediaCodecHelper.isExperimentalVendorDecoder("OMX.Nvidia.h264.decode"));
        assertTrue(MediaCodecHelper.isExperimentalVendorDecoder("OMX.amlogic.hevc.decoder.awesome"));
        assertTrue(MediaCodecHelper.isExperimentalVendorDecoder("c2.qcom.hevc.decoder"));
        assertTrue(MediaCodecHelper.isExperimentalVendorDecoder("c2.exynos.h264.decoder"));
        assertTrue(MediaCodecHelper.isExperimentalVendorDecoder("OMX.Exynos.avc.dec"));
        assertTrue(MediaCodecHelper.isExperimentalVendorDecoder("c2.google.hevc.decoder"));
        assertFalse(MediaCodecHelper.isExperimentalVendorDecoder("c2.qti.hevc.decoder.low_latency"));
        assertFalse(MediaCodecHelper.isExperimentalVendorDecoder("c2.android.hevc.decoder"));
    }

    private static MediaCodecInfo decoder(String name, String... declaredVendorParameters) {
        Map<String, List<String>> parameters = ReflectionHelpers.getStaticField(
                MediaCodecHelper.class, "vendorParameterCache");
        parameters.put(name, Arrays.asList(declaredVendorParameters));
        MediaFormat capabilities = MediaFormat.createVideoFormat(HEVC, 1920, 1080);
        return MediaCodecInfoBuilder.newBuilder().setName(name).setIsHardwareAccelerated(true).setIsSoftwareOnly(false)
                .setCapabilities(
                CodecCapabilitiesBuilder.newBuilder().setMediaFormat(capabilities)
                        .setProfileLevels(new CodecProfileLevel[0])
                        .setColorFormats(new int[] {CodecCapabilities.COLOR_FormatYUV420Flexible}).build()).build();
    }
}

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
 * Moonlight fork, GPL-3.0); every key goes only to the decoders it belongs to.
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
            "vendor.mtk.vdec.vsync.adjust.enable"};

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
            assertFalse(name, format.containsKey(MediaFormat.KEY_OPERATING_RATE));
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
    public void mediaTekDecodersWithoutDeclaredParametersGetTheWholeSetByName() {
        for (String name : new String[] {"c2.mtk.hevc.decoder", "OMX.MTK.VIDEO.DECODER.HEVC"}) {
            MediaFormat format = tune(name, 0, false);
            assertKeys(name, format, MTK_KEYS);
            assertEquals(name, 0, format.getInteger("vendor.mtk.vdec.vsync.adjust.enable"));
            assertEquals(name, name.startsWith("OMX"), format.containsKey("vdec-lowlatency"));
            assertAbsent(name, format, QTI_KEYS);
            assertAbsent(name, format, TEGRA_KEYS);
            assertFalse(name, format.containsKey(MediaFormat.KEY_OPERATING_RATE));
        }
    }

    @Test
    public void mediaTekKeysThatBoostClocksDropOutOnTheFallbackTries() {
        MediaFormat last = tune("c2.mtk.hevc.decoder", 4, false);
        assertAbsent("try 4", last, MTK_KEYS);
    }

    @Test
    @Config(sdk = 34)
    public void androidTwelveSendsOnlyTheMediaTekKeysTheDecoderDeclares() {
        MediaCodecInfo decoder = decoder("c2.mtk.hevc.decoder", "vendor.mtk.vdec.low-latency.mode",
                "vendor.mtk.vdec.disable-idle", "vendor.mtk.vdec.unrelated.value");
        MediaFormat format = MediaFormat.createVideoFormat(HEVC, 1920, 1080);
        assertTrue(MediaCodecHelper.setDecoderLowLatencyOptions(format, decoder, 0, true, true, false, false));
        assertKeys("declared", format, "vendor.mtk.vdec.low-latency.mode", "vendor.mtk.vdec.disable-idle");
        assertAbsent("undeclared", format, "vendor.mtk.vdec.dvfs.mode", "vendor.mtk.vdec.dvfs.level",
                "vendor.mtk.vdec.vsync.adjust.enable", "vendor.mtk.vdec.unrelated.value");
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
        assertTrue(firstKeys.contains("vendor.mtk.vdec.dvfs.mode=1"));
        assertTrue(firstKeys.contains("vendor.mtk.vdec.vsync.adjust.enable=0"));
        assertTrue(firstKeys.contains("low-latency=1"));
        assertTrue(lastKeys.isEmpty());
        List<String> dropped = MediaCodecHelper.droppedTuningKeys(firstKeys, lastKeys);
        assertTrue(dropped.contains("vendor.mtk.vdec.dvfs.mode=1"));
        assertTrue(MediaCodecHelper.droppedTuningKeys(firstKeys, firstKeys).isEmpty());
    }

    private static MediaFormat tune(String name, int attempt, boolean performanceHints) {
        MediaFormat format = MediaFormat.createVideoFormat(HEVC, 1920, 1080);
        MediaCodecHelper.setDecoderLowLatencyOptions(format, decoder(name), attempt, true, true, performanceHints, false);
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

    private static MediaCodecInfo decoder(String name, String... declaredVendorParameters) {
        Map<String, List<String>> parameters = ReflectionHelpers.getStaticField(
                MediaCodecHelper.class, "vendorParameterCache");
        parameters.put(name, Arrays.asList(declaredVendorParameters));
        MediaFormat capabilities = MediaFormat.createVideoFormat(HEVC, 1920, 1080);
        return MediaCodecInfoBuilder.newBuilder().setName(name).setCapabilities(
                CodecCapabilitiesBuilder.newBuilder().setMediaFormat(capabilities)
                        .setProfileLevels(new CodecProfileLevel[0])
                        .setColorFormats(new int[] {CodecCapabilities.COLOR_FormatYUV420Flexible}).build()).build();
    }
}

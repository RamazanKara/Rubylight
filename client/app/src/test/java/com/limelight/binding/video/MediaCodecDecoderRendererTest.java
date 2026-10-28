package com.limelight.binding.video;

import android.media.MediaCodecInfo.CodecProfileLevel;

import org.junit.Test;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;

import static com.limelight.nvstream.jni.MoonBridge.*;
import static org.junit.Assert.*;

public class MediaCodecDecoderRendererTest {
    @Test
    public void pyroWaveOverlayDistinguishesRecordLossAndMissingMeasurements() {
        java.util.Locale previous = java.util.Locale.getDefault();
        try {
            java.util.Locale.setDefault(java.util.Locale.US);
            assertEquals("PyroWave records missing: 2.50%\nQueue: 3.25 ms | GPU decode (last): 4.25 ms",
                    MediaCodecDecoderRenderer.formatPyroWaveStats(2.5f, 3.25f, 4250));
            String missing = MediaCodecDecoderRenderer.formatPyroWaveStats(-1, Float.NaN, 0);
            assertFalse(missing.contains("NaN"));
            assertFalse(missing.contains("0.00"));
            assertFalse(missing.contains("GPU decode (last)"));
            assertTrue(MediaCodecDecoderRenderer.formatPyroWaveStats(0, 0, 1).contains("missing: 0.00%"));
        } finally {
            java.util.Locale.setDefault(previous);
        }
    }

    @Test
    public void outputUsesFirstVsyncWhosePresentationDeadlineIsStillAhead() {
        long vsyncNs = 1000000000L;
        long intervalNs = 10000000L;
        long deadlineNs = 2000000L;
        assertEquals(vsyncNs + intervalNs, MediaCodecDecoderRenderer.nextVsyncTimeNs(
                vsyncNs + 7999999L, vsyncNs, intervalNs, deadlineNs));
        assertEquals(vsyncNs + 2 * intervalNs, MediaCodecDecoderRenderer.nextVsyncTimeNs(
                vsyncNs + 8000000L, vsyncNs, intervalNs, deadlineNs));
        assertEquals(vsyncNs + 2 * intervalNs, MediaCodecDecoderRenderer.nextVsyncTimeNs(
                vsyncNs + 9000000L, vsyncNs, intervalNs, deadlineNs));
    }

    @Test
    public void latchSlackIsTheWaitUntilTheNextDeadline() {
        long vsyncNs = 1000000000L;
        long intervalNs = 10000000L;
        long deadlineNs = 2000000L;
        // Deadlines fall 2 ms before each vsync: at +8 ms, +18 ms, ...
        assertEquals(3000000L, MediaCodecDecoderRenderer.latchSlackNs(vsyncNs + 5000000L, vsyncNs, intervalNs, deadlineNs));
        assertEquals(1L, MediaCodecDecoderRenderer.latchSlackNs(vsyncNs + 7999999L, vsyncNs, intervalNs, deadlineNs));
        // Just missing a deadline waits almost a whole refresh for the next one.
        assertEquals(intervalNs, MediaCodecDecoderRenderer.latchSlackNs(vsyncNs + 8000000L, vsyncNs, intervalNs, deadlineNs));
        assertEquals(-1L, MediaCodecDecoderRenderer.latchSlackNs(vsyncNs, 0, intervalNs, deadlineNs));
    }

    @Test
    public void delayedChoreographerCallbacksSkipExpiredVsyncs() {
        assertEquals(1060000000L, MediaCodecDecoderRenderer.nextVsyncTimeNs(
                1059000000L, 1000000000L, 10000000L, 500000L));
        assertEquals(1080000000L, MediaCodecDecoderRenderer.nextVsyncTimeNs(
                1059000000L, 1000000000L, 10000000L, 20000000L));
    }

    @Test
    public void outputTracksDisplayRateRatherThanStreamRate() {
        long vsyncNs = 1000000000L;
        long nowNs = vsyncNs + 7000000L;
        assertEquals(vsyncNs + 16666666L, MediaCodecDecoderRenderer.nextVsyncTimeNs(
                nowNs, vsyncNs, 16666666L, 2000000L));
        assertEquals(vsyncNs + 2 * 8333333L, MediaCodecDecoderRenderer.nextVsyncTimeNs(
                nowNs, vsyncNs, 8333333L, 2000000L));
        assertEquals(vsyncNs + 11111111L, MediaCodecDecoderRenderer.nextVsyncTimeNs(
                nowNs, vsyncNs, 11111111L, 2000000L));
    }

    @Test
    public void sixtyHzRenderOverrideOn120HzPanelDoesNotCollapseConsecutiveFrames() {
        long vsyncNs = 1000000000L;
        long panelIntervalNs = (long) (1000000000.0 / 120);
        long renderIntervalNs = (long) (1000000000.0 / 60);
        long firstFrameNs = vsyncNs + 1000000L;
        long secondFrameNs = firstFrameNs + panelIntervalNs;
        assertEquals(vsyncNs + panelIntervalNs, MediaCodecDecoderRenderer.nextVsyncTimeNs(
                firstFrameNs, vsyncNs, panelIntervalNs, 2000000L));
        assertEquals(vsyncNs + 2 * panelIntervalNs, MediaCodecDecoderRenderer.nextVsyncTimeNs(
                secondFrameNs, vsyncNs, panelIntervalNs, 2000000L));
        assertEquals(MediaCodecDecoderRenderer.nextVsyncTimeNs(firstFrameNs, vsyncNs, renderIntervalNs, 2000000L),
                MediaCodecDecoderRenderer.nextVsyncTimeNs(secondFrameNs, vsyncNs, renderIntervalNs, 2000000L));
    }

    @Test
    public void missingVsyncSampleDoesNotInventAPresentationTime() {
        assertEquals(1234L, MediaCodecDecoderRenderer.nextVsyncTimeNs(1234L, 0, 16666666L, 2000000L));
        assertEquals(1234L, MediaCodecDecoderRenderer.nextVsyncTimeNs(1234L, 1000L, 0, 2000000L));
    }

    @Test
    public void performanceHintsAreANoOpBelowAndroid12() {
        try (DecoderPerformanceHints hints = new DecoderPerformanceHints(null, true, 60)) {
            hints.reportWorkDuration(1000000L, 60);
            hints.reportWorkDuration(2000000L, 120);
        }
    }

    @Test
    public void main10AloneIsNotAnHdrDisplayPipeline() {
        assertEquals(VIDEO_FORMAT_H265, MediaCodecDecoderRenderer.videoFormatsForProfiles("video/hevc",
                new int[] {CodecProfileLevel.HEVCProfileMain10}, true, true));
        assertEquals(VIDEO_FORMAT_AV1_MAIN8, MediaCodecDecoderRenderer.videoFormatsForProfiles("video/av01",
                new int[] {CodecProfileLevel.AV1ProfileMain10}, true, true));
    }

    @Test
    public void plainMain10GivesTenBitFormatsOnlyForTenBitSdr() {
        assertEquals(VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10, MediaCodecDecoderRenderer.videoFormatsForProfiles(
                "video/hevc", new int[] {CodecProfileLevel.HEVCProfileMain10}, false, true, false));
        assertEquals(VIDEO_FORMAT_AV1_MAIN8 | VIDEO_FORMAT_AV1_MAIN10, MediaCodecDecoderRenderer.videoFormatsForProfiles(
                "video/av01", new int[] {CodecProfileLevel.AV1ProfileMain10}, false, true, false));
        // The HDR10 profiles are Main10 decoders too.
        assertEquals(VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10, MediaCodecDecoderRenderer.videoFormatsForProfiles(
                "video/hevc", new int[] {CodecProfileLevel.HEVCProfileMain10HDR10}, false, true, false));
        // Without the 10-bit SDR request the old rules hold.
        assertEquals(VIDEO_FORMAT_H265, MediaCodecDecoderRenderer.videoFormatsForProfiles(
                "video/hevc", new int[] {CodecProfileLevel.HEVCProfileMain10}, false, false, false));
        assertEquals(VIDEO_FORMAT_AV1_MAIN8, MediaCodecDecoderRenderer.videoFormatsForProfiles(
                "video/av01", new int[] {CodecProfileLevel.AV1ProfileMain10}, false, false, false));
        assertEquals(VIDEO_FORMAT_H265, MediaCodecDecoderRenderer.videoFormatsForProfiles(
                "video/hevc", new int[] {CodecProfileLevel.HEVCProfileMain}, false, true, false));
    }

    @Test
    public void hdr10AndHdr10PlusDecodersCanReceiveStaticHdr10() {
        for (int profile : new int[] {CodecProfileLevel.HEVCProfileMain10HDR10, CodecProfileLevel.HEVCProfileMain10HDR10Plus}) {
            assertEquals(VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10,
                    MediaCodecDecoderRenderer.videoFormatsForProfiles("video/hevc", new int[] {profile}, true, false));
        }
        for (int profile : new int[] {CodecProfileLevel.AV1ProfileMain10HDR10, CodecProfileLevel.AV1ProfileMain10HDR10Plus}) {
            assertEquals(VIDEO_FORMAT_AV1_MAIN8 | VIDEO_FORMAT_AV1_MAIN10,
                    MediaCodecDecoderRenderer.videoFormatsForProfiles("video/av01", new int[] {profile}, true, false));
        }
    }

    @Test
    public void sdrDisplayNeverAdvertisesHdr() {
        assertEquals(VIDEO_FORMAT_H265, MediaCodecDecoderRenderer.videoFormatsForProfiles("video/hevc",
                new int[] {CodecProfileLevel.HEVCProfileMain10HDR10}, false, false));
    }

    @Test
    public void onlyExplicit444ProfilesAdvertise444() {
        assertEquals(VIDEO_FORMAT_H264, MediaCodecDecoderRenderer.videoFormatsForProfiles("video/avc",
                new int[] {CodecProfileLevel.AVCProfileHigh}, false, true));
        assertEquals(VIDEO_FORMAT_H264 | VIDEO_FORMAT_H264_HIGH8_444,
                MediaCodecDecoderRenderer.videoFormatsForProfiles("video/avc",
                        new int[] {CodecProfileLevel.AVCProfileHigh444}, false, true));
        assertEquals(VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_REXT8_444,
                MediaCodecDecoderRenderer.videoFormatsForProfiles("video/hevc",
                        new int[] {CodecProfileLevel.HEVCProfileMain444}, false, true));
        assertEquals(VIDEO_FORMAT_H265, MediaCodecDecoderRenderer.videoFormatsForProfiles("video/hevc",
                new int[] {CodecProfileLevel.HEVCProfileMain444}, false, false));
    }

    @Test
    public void av1MainAndHevcMain10CannotClaimHdr444() {
        int formats = MediaCodecDecoderRenderer.videoFormatsForProfiles("video/av01",
                new int[] {CodecProfileLevel.AV1ProfileMain10HDR10}, true, true) |
                MediaCodecDecoderRenderer.videoFormatsForProfiles("video/hevc",
                        new int[] {CodecProfileLevel.HEVCProfileMain10HDR10}, true, true);
        assertEquals(0, formats & VIDEO_FORMAT_MASK_YUV444);
    }

    @Test
    public void hostMetadataKeepsWireUnitsAndExcludesFullFrameExtension() {
        // rust/core/src/hdr.rs: Metadata::display(1499.9, 0.002, 800.0).wire(true), excluding enabled.
        int[] values = {35400, 14600, 8500, 39850, 6550, 2300, 15635, 16450, 1499, 20, 0, 0, 800};
        ByteBuffer wire = ByteBuffer.allocate(26).order(ByteOrder.LITTLE_ENDIAN);
        for (int value : values) {
            wire.putShort((short) value);
        }
        ByteBuffer info = MediaCodecDecoderRenderer.createHdrStaticInfo(wire.array()).order(ByteOrder.LITTLE_ENDIAN);
        assertEquals(25, info.remaining());
        assertEquals(0, info.get());
        for (int i = 0; i < 12; i++) {
            assertEquals(values[i], Short.toUnsignedInt(info.getShort()));
        }
        assertFalse(info.hasRemaining());
    }

    @Test
    public void missingOrTruncatedMetadataIsNotPassedToMediaCodec() {
        assertNull(MediaCodecDecoderRenderer.createHdrStaticInfo(null));
        assertNull(MediaCodecDecoderRenderer.createHdrStaticInfo(new byte[23]));
        assertEquals(25, MediaCodecDecoderRenderer.createHdrStaticInfo(new byte[24]).remaining());
    }

    @Test
    public void hdrMetadataAloneIsAParameterOnlyForDecodersThatTakeIt() {
        // The PC's first HDR report on the TCL C6K: HDR was assumed on, and the mastering metadata
        // arrives. A stop() and configure() then left its decoder without input buffers.
        byte[] first = new byte[24];
        first[0] = 1;
        byte[] next = first.clone();
        next[23] = 2;
        int q = android.os.Build.VERSION_CODES.Q;
        assertEquals(MediaCodecDecoderRenderer.HdrChange.PARAMETERS,
                MediaCodecDecoderRenderer.hdrChange(true, true, null, first, q, true));
        assertEquals(MediaCodecDecoderRenderer.HdrChange.PARAMETERS,
                MediaCodecDecoderRenderer.hdrChange(true, true, first, next, q + 5, true));
        // Every other decoder restarts, as before: an OMX decoder may take the parameter and ignore it.
        assertEquals(MediaCodecDecoderRenderer.HdrChange.RESTART,
                MediaCodecDecoderRenderer.hdrChange(true, true, null, first, q, false));
        assertEquals(MediaCodecDecoderRenderer.HdrChange.RESTART,
                MediaCodecDecoderRenderer.hdrChange(true, true, first, next, q + 5, false));
        for (boolean asParameter : new boolean[] {true, false}) {
            assertEquals(MediaCodecDecoderRenderer.HdrChange.NONE,
                    MediaCodecDecoderRenderer.hdrChange(true, true, first, first.clone(), q, asParameter));
            assertEquals(MediaCodecDecoderRenderer.HdrChange.NONE,
                    MediaCodecDecoderRenderer.hdrChange(false, false, null, null, q, asParameter));
        }
    }

    @Test
    public void hdrOnOrOffAndMetadataSetParametersCannotCarryRestartTheDecoder() {
        byte[] metadata = new byte[24];
        int q = android.os.Build.VERSION_CODES.Q;
        // The colour keys change with the mode.
        assertEquals(MediaCodecDecoderRenderer.HdrChange.RESTART,
                MediaCodecDecoderRenderer.hdrChange(false, true, null, metadata, q, true));
        assertEquals(MediaCodecDecoderRenderer.HdrChange.RESTART,
                MediaCodecDecoderRenderer.hdrChange(false, true, null, null, q, true));
        assertEquals(MediaCodecDecoderRenderer.HdrChange.RESTART,
                MediaCodecDecoderRenderer.hdrChange(true, false, metadata, null, q, true));
        // Before Android 10 setParameters does not take it, and it can't remove it.
        assertEquals(MediaCodecDecoderRenderer.HdrChange.RESTART,
                MediaCodecDecoderRenderer.hdrChange(true, true, null, metadata, q - 1, true));
        assertEquals(MediaCodecDecoderRenderer.HdrChange.RESTART,
                MediaCodecDecoderRenderer.hdrChange(true, true, metadata, null, q, true));
    }

    @Test
    public void onlyMediaTekCodec2DecodersTakeHdrMetadataAsAParameter() {
        assertTrue(MediaCodecDecoderRenderer.takesHdrStaticInfoParameter("c2.mtk.dvhe.st.decoder"));
        assertTrue(MediaCodecDecoderRenderer.takesHdrStaticInfoParameter("c2.mtk.av1.decoder"));
        assertTrue(MediaCodecDecoderRenderer.takesHdrStaticInfoParameter("C2.MTK.hevc.decoder"));
        for (String other : new String[] {"OMX.Nvidia.h265.decode", "OMX.MTK.VIDEO.DECODER.HEVC",
                "c2.qti.hevc.decoder", "c2.exynos.hevc.decoder", "OMX.amlogic.hevc.decoder.awesome2",
                "c2.android.hevc.decoder"}) {
            assertFalse(other, MediaCodecDecoderRenderer.takesHdrStaticInfoParameter(other));
        }
    }

    @Test
    public void mediaTekWithItsVendorLowLatencyModeRestartsAsANewCodec() {
        java.util.List<String> vendor = java.util.Arrays.asList("low-latency=1",
                "vendor.mtk.vdec.low-latency.mode=1", "vendor.mtk.vdec.low-latency.mode.value=1");
        java.util.List<String> standard = java.util.Arrays.asList("low-latency=1", "priority=0");
        assertTrue(MediaCodecDecoderRenderer.restartRecreates("c2.mtk.dvhe.st.decoder", vendor));
        assertTrue(MediaCodecDecoderRenderer.restartRecreates("c2.mtk.av1.decoder", vendor));
        // Run b4lll: without the vendor mode stop() and configure() did not hang.
        assertFalse(MediaCodecDecoderRenderer.restartRecreates("c2.mtk.av1.decoder", standard));
        assertFalse(MediaCodecDecoderRenderer.restartRecreates("c2.mtk.av1.decoder",
                java.util.Collections.singletonList("vendor.mtk.vdec.low-latency.mode=0")));
        assertFalse(MediaCodecDecoderRenderer.restartRecreates("c2.mtk.av1.decoder",
                java.util.Collections.<String>emptyList()));
        // Other decoders keep stop() and configure().
        assertFalse(MediaCodecDecoderRenderer.restartRecreates("OMX.MTK.VIDEO.DECODER.HEVC", vendor));
        assertFalse(MediaCodecDecoderRenderer.restartRecreates("c2.qti.hevc.decoder",
                java.util.Arrays.asList("low-latency=1", "vendor.qti-ext-dec-low-latency.enable=1")));
    }

    /** The output format's hdr-static-info c2.mtk.dvhe.st.decoder showed in run b1fix. */
    private static final int[] B1FIX_OUTPUT = {0x00, 0x48, 0x8a, 0x08, 0x39, 0x34, 0x21, 0xaa, 0x9b, 0x96, 0x19,
            0xfc, 0x08, 0x13, 0x3d, 0x42, 0x40, 0x90, 0x01, 0x09, 0x00, 0x00, 0x00, 0x00, 0x00};

    private static byte[] bytes(int[] values) {
        byte[] out = new byte[values.length];
        for (int i = 0; i < values.length; i++) out[i] = (byte) values[i];
        return out;
    }

    @Test
    public void theOutputFormatCarriesTheMetadataToWithinCodec2Rounding() {
        byte[] wire = java.util.Arrays.copyOfRange(bytes(B1FIX_OUTPUT), 1, 25);
        byte[] expected = MediaCodecDecoderRenderer.createHdrStaticInfo(wire).array();
        assertTrue(MediaCodecDecoderRenderer.carriesHdrStaticInfo(ByteBuffer.wrap(bytes(B1FIX_OUTPUT)), expected));
        // Read from the buffer's position, which is left where it was.
        ByteBuffer offset = ByteBuffer.allocate(30);
        offset.position(3);
        offset.put(bytes(B1FIX_OUTPUT));
        offset.position(3);
        assertTrue(MediaCodecDecoderRenderer.carriesHdrStaticInfo(offset, expected));
        assertEquals(3, offset.position());
        // red.x 35400 back from the float as 35399, and white.y 16450 as 16451.
        byte[] rounded = bytes(B1FIX_OUTPUT);
        rounded[1] = 0x47;
        rounded[15] = 0x43;
        assertTrue(MediaCodecDecoderRenderer.carriesHdrStaticInfo(ByteBuffer.wrap(rounded), expected));
    }

    @Test
    public void anOutputFormatWithoutTheMetadataDoesNotCarryIt() {
        byte[] expected = MediaCodecDecoderRenderer.createHdrStaticInfo(
                java.util.Arrays.copyOfRange(bytes(B1FIX_OUTPUT), 1, 25)).array();
        assertFalse(MediaCodecDecoderRenderer.carriesHdrStaticInfo(null, expected));
        // c2.mtk.dvhe.st.decoder's output format before the parameter: all zeros.
        assertFalse(MediaCodecDecoderRenderer.carriesHdrStaticInfo(ByteBuffer.allocate(25), expected));
        // An empty or a short buffer.
        assertFalse(MediaCodecDecoderRenderer.carriesHdrStaticInfo(ByteBuffer.allocate(0), expected));
        assertFalse(MediaCodecDecoderRenderer.carriesHdrStaticInfo(
                ByteBuffer.wrap(java.util.Arrays.copyOf(bytes(B1FIX_OUTPUT), 24)), expected));
        // Off by two in max luminance (400 nits), or another descriptor.
        byte[] luminance = bytes(B1FIX_OUTPUT);
        luminance[17] = (byte) 0x92;
        assertFalse(MediaCodecDecoderRenderer.carriesHdrStaticInfo(ByteBuffer.wrap(luminance), expected));
        byte[] descriptor = bytes(B1FIX_OUTPUT);
        descriptor[0] = 1;
        assertFalse(MediaCodecDecoderRenderer.carriesHdrStaticInfo(ByteBuffer.wrap(descriptor), expected));
    }
}

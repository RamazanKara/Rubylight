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
}

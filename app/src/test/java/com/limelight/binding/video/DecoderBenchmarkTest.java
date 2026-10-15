package com.limelight.binding.video;

import com.limelight.utils.DeviceResults;

import org.junit.Test;

import java.util.Arrays;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

import static org.junit.Assert.*;

public class DecoderBenchmarkTest {
    @Test public void clearlySlowerCodecsAreSkippedButNeverH264() {
        boolean[] skip = DecoderBenchmark.slower(new long[] {2000, 2300, 4000}, false);
        assertFalse(skip[0]);
        assertFalse(skip[1]);
        assertTrue(skip[2]);
        assertFalse(DecoderBenchmark.slower(new long[] {1000, 5000, 5000}, false)[0]);
    }

    @Test public void smallDifferencesKeepEveryCodec() {
        boolean[] skip = DecoderBenchmark.slower(new long[] {3000, 3600, 3400}, false);
        assertFalse(skip[1]);
        assertFalse(skip[2]);
    }

    @Test public void hdrComparesOnlyHdrCapableCodecs() {
        boolean[] skip = DecoderBenchmark.slower(new long[] {1000, 3000, 2500}, true);
        assertFalse(skip[1]);
        assertFalse(skip[2]);
    }

    @Test public void unmeasuredCodecsAreKept() {
        boolean[] skip = DecoderBenchmark.slower(new long[] {2000, 0, 0}, false);
        assertFalse(skip[1]);
        assertFalse(skip[2]);
    }

    @Test public void storedResultsBecomeExportEntriesInCodecOrder() {
        Map<String, Object> stored = new HashMap<>();
        stored.put("key", "fingerprint|331");
        stored.put("c2.mtk.hevc.decoder|video/hevc", 3100L);
        stored.put("c2.mtk.avc.decoder|video/avc", 2500L);
        stored.put("c2.mtk.av1.decoder|video/av01", 4200L);
        stored.put(DecoderBenchmark.FEATURE_PREFIX + "video/avc|c2.mtk.avc.decoder", true);
        stored.put(DecoderBenchmark.VENDOR_KEYS_PREFIX + "video/avc|c2.mtk.avc.decoder",
                "vendor.mtk.vdec.low-latency.mode,vendor.mtk.vdec.cpu.boost.mode");
        stored.put(DecoderBenchmark.FEATURE_PREFIX + "video/hevc|c2.mtk.hevc.decoder", false);
        stored.put(DecoderBenchmark.VENDOR_KEYS_PREFIX + "video/hevc|c2.mtk.hevc.decoder", "");

        List<DeviceResults.Decoder> decoders = DecoderBenchmark.decoders(stored);

        assertEquals(3, decoders.size());
        DeviceResults.Decoder avc = decoders.get(0);
        assertEquals("H.264", avc.codec);
        assertEquals("video/avc", avc.mime);
        assertEquals("c2.mtk.avc.decoder", avc.name);
        assertEquals(2500L, avc.medianDecodeUs);
        assertEquals(Boolean.TRUE, avc.featureLowLatency);
        assertEquals(Arrays.asList("vendor.mtk.vdec.low-latency.mode", "vendor.mtk.vdec.cpu.boost.mode"),
                avc.vendorLowLatencyKeys);
        DeviceResults.Decoder hevc = decoders.get(1);
        assertEquals("HEVC", hevc.codec);
        assertEquals(Boolean.FALSE, hevc.featureLowLatency);
        assertTrue(hevc.vendorLowLatencyKeys.isEmpty());
        DeviceResults.Decoder av1 = decoders.get(2);
        assertEquals("AV1", av1.codec);
        assertNull(av1.featureLowLatency);
        assertNull(av1.vendorLowLatencyKeys);
    }

    @Test public void detailEntriesAreNeverMistakenForTimings() {
        Map<String, Object> stored = new HashMap<>();
        stored.put(DecoderBenchmark.FEATURE_PREFIX + "video/avc|c2.qti.avc.decoder", true);
        stored.put(DecoderBenchmark.VENDOR_KEYS_PREFIX + "video/avc|c2.qti.avc.decoder", "vendor.x.enable");
        stored.put("|video/avc", 1000L);
        assertTrue(DecoderBenchmark.decoders(stored).isEmpty());
    }
}

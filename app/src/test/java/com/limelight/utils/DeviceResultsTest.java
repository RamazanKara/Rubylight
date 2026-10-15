package com.limelight.utils;

import org.junit.Test;

import java.util.Arrays;
import java.util.Collections;

import static org.junit.Assert.*;

public class DeviceResultsTest {
    private static DeviceResults sample() {
        DeviceResults results = new DeviceResults();
        results.appVersion = "0.6.0";
        results.appBuild = 331;
        results.manufacturer = "samsung";
        results.model = "SM-S918B";
        results.codename = "dm3q";
        results.socManufacturer = "QTI";
        results.socModel = "SM8550";
        results.hardware = "qcom";
        results.board = "kalama";
        results.androidRelease = "14";
        results.sdk = 34;
        results.gpu = "Adreno (TM) 740";
        results.decoderBenchmarkMeasured = true;
        results.clipWidth = 1920;
        results.clipHeight = 1080;
        results.clipFps = 60;
        results.decoders.add(new DeviceResults.Decoder("H.264", "video/avc", "c2.qti.avc.decoder", 2104, true,
                Arrays.asList("vendor.qti-ext-dec-picture-order.enable", "vendor.qti-ext-dec-low-latency.enable")));
        results.decoders.add(new DeviceResults.Decoder("HEVC", "video/hevc", "c2.qti.hevc.decoder", 2450, null, null));
        DeviceResults.Latency latency = new DeviceResults.Latency();
        latency.panelHz = 120;
        latency.audioSampleRate = 48000;
        latency.audioFramesPerBuffer = 192;
        latency.metrics[0] = new double[] {1199, 8.333, 8.4, 9.125};
        latency.metrics[1] = new double[] {1200, 0.5, 1.25, 2};
        latency.metrics[2] = new double[] {0, -1, -1, -1};
        latency.metrics[3] = new double[] {100, 0.75, 1.5, 3.25};
        results.latency = latency;
        return results;
    }

    @Test public void fullExportMatchesTheVersionOneLayout() {
        assertEquals("{\n"
                + "  \"format\": \"nightjar-device-results\",\n"
                + "  \"version\": 1,\n"
                + "  \"app\": {\n"
                + "    \"name\": \"Nightjar\",\n"
                + "    \"version\": \"0.6.0\",\n"
                + "    \"build\": 331\n"
                + "  },\n"
                + "  \"device\": {\n"
                + "    \"manufacturer\": \"samsung\",\n"
                + "    \"model\": \"SM-S918B\",\n"
                + "    \"codename\": \"dm3q\",\n"
                + "    \"socManufacturer\": \"QTI\",\n"
                + "    \"socModel\": \"SM8550\",\n"
                + "    \"hardware\": \"qcom\",\n"
                + "    \"board\": \"kalama\",\n"
                + "    \"android\": \"14\",\n"
                + "    \"sdk\": 34,\n"
                + "    \"gpu\": \"Adreno (TM) 740\"\n"
                + "  },\n"
                + "  \"decoderBenchmark\": {\n"
                + "    \"measured\": true,\n"
                + "    \"clip\": \"1920x1080@60\",\n"
                + "    \"results\": [\n"
                + "      {\n"
                + "        \"codec\": \"H.264\",\n"
                + "        \"mime\": \"video/avc\",\n"
                + "        \"decoder\": \"c2.qti.avc.decoder\",\n"
                + "        \"medianDecodeMs\": 2.10,\n"
                + "        \"featureLowLatency\": true,\n"
                + "        \"vendorLowLatencyKeys\": [\n"
                + "          \"vendor.qti-ext-dec-low-latency.enable\",\n"
                + "          \"vendor.qti-ext-dec-picture-order.enable\"\n"
                + "        ]\n"
                + "      },\n"
                + "      {\n"
                + "        \"codec\": \"HEVC\",\n"
                + "        \"mime\": \"video/hevc\",\n"
                + "        \"decoder\": \"c2.qti.hevc.decoder\",\n"
                + "        \"medianDecodeMs\": 2.45,\n"
                + "        \"featureLowLatency\": null,\n"
                + "        \"vendorLowLatencyKeys\": null\n"
                + "      }\n"
                + "    ]\n"
                + "  },\n"
                + "  \"latencyTest\": {\n"
                + "    \"panelHz\": 120.00,\n"
                + "    \"audioSampleRate\": 48000,\n"
                + "    \"audioFramesPerBuffer\": 192,\n"
                + "    \"frameIntervalMs\": {\n"
                + "      \"samples\": 1199,\n"
                + "      \"mean\": 8.33,\n"
                + "      \"p95\": 8.40,\n"
                + "      \"p99\": 9.13\n"
                + "    },\n"
                + "    \"frameCallbackDelayMs\": {\n"
                + "      \"samples\": 1200,\n"
                + "      \"mean\": 0.50,\n"
                + "      \"p95\": 1.25,\n"
                + "      \"p99\": 2.00\n"
                + "    },\n"
                + "    \"inputDispatchAgeMs\": null,\n"
                + "    \"timerWakeupDelayMs\": {\n"
                + "      \"samples\": 100,\n"
                + "      \"mean\": 0.75,\n"
                + "      \"p95\": 1.50,\n"
                + "      \"p99\": 3.25\n"
                + "    }\n"
                + "  }\n"
                + "}", sample().toJson());
    }

    @Test public void unmeasuredDeviceStillExportsItsIdentityWithNulls() {
        DeviceResults results = new DeviceResults();
        results.appVersion = "0.6.0";
        results.appBuild = 331;
        results.manufacturer = "Amazon";
        results.model = "AFTKA";
        results.sdk = 28;
        assertEquals("{\n"
                + "  \"format\": \"nightjar-device-results\",\n"
                + "  \"version\": 1,\n"
                + "  \"app\": {\n"
                + "    \"name\": \"Nightjar\",\n"
                + "    \"version\": \"0.6.0\",\n"
                + "    \"build\": 331\n"
                + "  },\n"
                + "  \"device\": {\n"
                + "    \"manufacturer\": \"Amazon\",\n"
                + "    \"model\": \"AFTKA\",\n"
                + "    \"codename\": null,\n"
                + "    \"socManufacturer\": null,\n"
                + "    \"socModel\": null,\n"
                + "    \"hardware\": null,\n"
                + "    \"board\": null,\n"
                + "    \"android\": null,\n"
                + "    \"sdk\": 28,\n"
                + "    \"gpu\": null\n"
                + "  },\n"
                + "  \"decoderBenchmark\": {\n"
                + "    \"measured\": false,\n"
                + "    \"results\": []\n"
                + "  },\n"
                + "  \"latencyTest\": null\n"
                + "}", results.toJson());
    }

    @Test public void addressesAccountsAndKnownPcNamesAreRedacted() {
        RedactedLog.addSecret("living-room-pc-results");
        DeviceResults results = sample();
        results.model = "Box 192.168.1.20 of living-room-pc-results";
        results.manufacturer = "owner@example.com";
        results.gpu = "Renderer 0123456789abcdef0123";
        results.codename = "fe80::1:2:3";
        String json = results.toJson();
        assertTrue(json.contains("\"model\": \"Box [address] of [PC]\""));
        assertTrue(json.contains("\"manufacturer\": \"[email]\""));
        assertTrue(json.contains("\"gpu\": \"Renderer [hex]\""));
        assertFalse(json.contains("fe80"));
        assertFalse(json.contains("192.168"));
        assertFalse(json.contains("example.com"));
        assertFalse(json.contains("living-room"));
    }

    @Test public void codecNamesAndVendorKeysMustLookLikeIdentifiers() {
        DeviceResults results = new DeviceResults();
        results.decoderBenchmarkMeasured = true;
        results.decoders.add(new DeviceResults.Decoder("AV1", "video/av01", "c2.vendor decoder \"x\"", 0, false,
                Arrays.asList("vendor.mtk.vdec.low-latency.mode", "not-vendor", "vendor.bad key",
                        "vendor.mtk.vdec.low-latency.mode")));
        String json = results.toJson();
        assertTrue(json.contains("\"decoder\": \"[omitted]\""));
        assertTrue(json.contains("\"medianDecodeMs\": null"));
        assertTrue(json.contains("\"featureLowLatency\": false"));
        assertTrue(json.contains("\"vendorLowLatencyKeys\": [\n          \"vendor.mtk.vdec.low-latency.mode\"\n        ]"));
        assertFalse(json.contains("not-vendor"));
        assertFalse(json.contains("bad key"));
    }

    @Test public void emptyKeyListIsAnEmptyArrayNotUnknown() {
        DeviceResults results = new DeviceResults();
        results.decoders.add(new DeviceResults.Decoder("H.264", "video/avc", "OMX.MTK.VIDEO.DECODER.AVC", 4000, false,
                Collections.emptyList()));
        assertTrue(results.toJson().contains("\"vendorLowLatencyKeys\": []"));
    }

    @Test public void freeTextIsEscapedSingleLineAndCapped() {
        DeviceResults results = new DeviceResults();
        results.model = "Quote \" back\\slash\ttab";
        results.gpu = "Line one\nline two";
        String board = new String(new char[40]).replace("\0", "board ");
        results.board = board;
        String json = results.toJson();
        assertTrue(json.contains("\"model\": \"Quote \\\" back\\\\slash\\u0009tab\""));
        assertTrue(json.contains("\"gpu\": \"Line one line two\""));
        assertTrue(json.contains("\"board\": \"" + board.substring(0, DeviceResults.MAX_TEXT) + "\""));
    }

    @Test public void audioPropertiesParseOnlyPositiveNumbers() {
        assertEquals(Integer.valueOf(48000), DeviceResults.Latency.audioProperty(" 48000 "));
        assertNull(DeviceResults.Latency.audioProperty(null));
        assertNull(DeviceResults.Latency.audioProperty("0"));
        assertNull(DeviceResults.Latency.audioProperty("unknown"));
    }

    @Test public void negativeOrMissingMeasurementsBecomeNull() {
        DeviceResults results = new DeviceResults();
        results.latency = new DeviceResults.Latency();
        results.latency.panelHz = Double.NaN;
        String json = results.toJson();
        assertTrue(json.contains("\"panelHz\": null"));
        assertTrue(json.contains("\"audioSampleRate\": null"));
        assertTrue(json.contains("\"frameIntervalMs\": null"));
    }

    @Test public void reportFormUrlOpensTheIssueFormWithoutDeviceData() {
        assertEquals("https://github.com/RamazanKara/nightjar/issues/new?template=device-results.yml",
                DeviceResults.REPORT_FORM_URL);
    }
}

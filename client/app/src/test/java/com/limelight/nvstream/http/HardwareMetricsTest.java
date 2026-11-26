package com.limelight.nvstream.http;

import android.app.Application;

import com.limelight.nativeapi.NativeApiFixtures;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.io.FileNotFoundException;
import java.io.IOException;
import java.util.ArrayList;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class HardwareMetricsTest {
    private static final String EMPTY = "{\"version\":1,\"sampled_at_unix_ms\":1791712800000," +
            "\"interval_ms\":1000,\"cpu\":{},\"gpus\":[]}";

    @Test
    public void optionalReadingsPreserveZeroAndDoNotInventMissingValues() throws Exception {
        HardwareMetrics sample = HardwareMetrics.parse(EMPTY.replace("\"cpu\":{}",
                "\"cpu\":{\"cpu_usage_percent\":41.6,\"future_cpu_metric\":99}").replace("\"gpus\":[]",
                "\"gpus\":[{\"id\":-12,\"name\":\"AMD Radeon\",\"metrics\":{" +
                        "\"gpu_usage_percent\":0,\"gpu_fan_rpm\":0,\"gpu_temperature_c\":61.5," +
                        "\"gpu_power_w\":null,\"gpu_clock_mhz\":\"1234\",\"gpu_voltage_mv\":-1," +
                        "\"gpu_memory_clock_mhz\":\"NaN\",\"future_gpu_metric\":123}}]"));
        assertEquals(1791712800000L, sample.sampledAtUnixMs);
        assertEquals(1000, sample.intervalMs);
        assertEquals(Collections.singletonMap("cpu_usage_percent", 41.6), sample.cpu);
        HardwareMetrics.Gpu gpu = sample.gpus.get(0);
        assertEquals(-12, gpu.id);
        assertEquals("AMD Radeon", gpu.name);
        assertEquals(3, gpu.metrics.size());
        assertEquals(0, gpu.metrics.get("gpu_usage_percent"), 0);
        assertEquals(0, gpu.metrics.get("gpu_fan_rpm"), 0);
        assertEquals(61.5, gpu.metrics.get("gpu_temperature_c"), 0);
        assertNull(gpu.metrics.get("gpu_power_w"));
        assertTrue(HardwareMetrics.parse(EMPTY).cpu.isEmpty());
        assertTrue(HardwareMetrics.parse(EMPTY).gpus.isEmpty());
    }

    @Test
    public void allPublishedGpuReadingsAndMultipleDevicesArePreserved() throws Exception {
        String[] keys = {"gpu_usage_percent", "gpu_clock_mhz", "gpu_memory_clock_mhz",
                "gpu_temperature_c", "gpu_hotspot_temperature_c", "gpu_memory_temperature_c",
                "gpu_voltage_mv", "gpu_fan_rpm", "gpu_fan_percent", "gpu_power_w",
                "gpu_board_power_w", "gpu_vram_used_mb", "gpu_vram_usage_percent", "gpu_encoder_percent"};
        StringBuilder readings = new StringBuilder();
        for (String key : keys) {
            if (readings.length() != 0) readings.append(',');
            readings.append('"').append(key).append("\":12.5");
        }
        HardwareMetrics sample = HardwareMetrics.parse(EMPTY.replace("\"gpus\":[]",
                "\"gpus\":[{\"id\":2147483647,\"name\":\"AMD 1\",\"metrics\":{" + readings +
                        "}},{\"id\":-2147483648,\"name\":\"\",\"metrics\":{}}]"));
        assertEquals(2, sample.gpus.size());
        for (String key : keys) assertEquals(12.5, sample.gpus.get(0).metrics.get(key), 0);
        assertTrue(sample.gpus.get(1).metrics.isEmpty());
    }

    @Test
    public void unknownSchemasAndInvalidRequiredFieldsCannotBecomeSamples() {
        assertThrows(HardwareMetrics.UnsupportedVersionException.class,
                () -> HardwareMetrics.parse(EMPTY.replace("\"version\":1", "\"version\":2")));
        for (String json : new String[] {
                "not json", "{}", EMPTY.replace("\"version\":1", "\"version\":\"1\""),
                EMPTY.replace("1791712800000", "-1"),
                EMPTY.replace("1791712800000", "9223372036854775808"),
                EMPTY.replace("1000", "499"), EMPTY.replace("1000", "10001"),
                EMPTY.replace("1000", "1000.5"), EMPTY.replace("\"cpu\":{}", "\"cpu\":[]"),
                EMPTY.replace("\"cpu\":{}", "\"cpu\":{\"cpu_usage_percent\":1e999}"),
                EMPTY.replace("\"gpus\":[]", "\"gpus\":{}"),
                EMPTY.replace("\"gpus\":[]", "\"gpus\":[{\"id\":1,\"name\":1,\"metrics\":{}}]"),
                EMPTY.replace("\"gpus\":[]", "\"gpus\":[{\"id\":2147483648,\"name\":\"\",\"metrics\":{}}]")
        }) {
            assertThrows(json, IOException.class, () -> HardwareMetrics.parse(json));
        }
    }

    @Test
    public void responseDeviceCountsAndNamesAreBounded() throws Exception {
        String gpu = "{\"id\":1,\"name\":\"\",\"metrics\":{}}";
        String devices = String.join(",", Collections.nCopies(64, gpu));
        assertEquals(64, HardwareMetrics.parse(EMPTY.replace("\"gpus\":[]", "\"gpus\":[" + devices + "]")).gpus.size());
        assertThrows(IOException.class, () -> HardwareMetrics.parse(
                EMPTY.replace("\"gpus\":[]", "\"gpus\":[" + devices + "," + gpu + "]")));
        String longName = String.join("", Collections.nCopies(513, "a"));
        assertThrows(IOException.class, () -> HardwareMetrics.parse(EMPTY.replace("\"gpus\":[]",
                "\"gpus\":[" + gpu.replace("\"name\":\"\"", "\"name\":\"" + longName + "\"") + "]")));
        assertThrows(IOException.class, () -> HardwareMetrics.parse(
                String.join("", Collections.nCopies(HardwareMetrics.MAX_RESPONSE_BYTES + 1, " "))));
    }

    @Test
    public void samplesOwnTheirCollectionsAndCannotBeMutatedByConsumers() {
        Map<String, Double> readings = new LinkedHashMap<>();
        readings.put("gpu_usage_percent", 30.0);
        HardwareMetrics.Gpu gpu = new HardwareMetrics.Gpu(1, "AMD", readings);
        ArrayList<HardwareMetrics.Gpu> gpus = new ArrayList<>();
        gpus.add(gpu);
        HardwareMetrics sample = new HardwareMetrics(1, 1000, readings, gpus);
        readings.clear();
        gpus.clear();
        assertEquals(1, sample.gpus.size());
        assertEquals(1, sample.cpu.size());
        assertEquals(1, gpu.metrics.size());
        assertThrows(UnsupportedOperationException.class, sample.gpus::clear);
        assertThrows(UnsupportedOperationException.class, sample.cpu::clear);
        assertThrows(UnsupportedOperationException.class, gpu.metrics::clear);
    }

    private static final String METRICS_CAPS = "<root status_code=\"200\"><uniqueid>host-1</uniqueid>" +
            "<state>SUNSHINE_SERVER_FREE</state><HttpsPort>47984</HttpsPort>" +
            "<RustHostVersion>3.1.0</RustHostVersion><PairStatus>1</PairStatus>" +
            "<HardwareMetricsVersion>1</HardwareMetricsVersion>" +
            "<Permission>" + ComputerDetails.PERMISSION_VIEW + "</Permission></root>";

    @Test
    public void networkUsesPinnedHttpsAndBoundedDeadline() throws Exception {
        try (TestHttpServer server = new TestHttpServer(req -> {
            assertEquals("https", req.scheme);
            assertEquals("/api/hardware-metrics", req.path);
            assertEquals("device", req.queryParam("uniqueid"));
            return TestHttpServer.Response.json(200, EMPTY);
        })) {
            NvHTTP pinned = server.newClient("device", true, NativeApiFixtures.crypto());
            pinned.getComputerDetails(METRICS_CAPS);
            assertEquals(1000, pinned.getHardwareMetrics().intervalMs);
            assertEquals(1, server.requests().size());

            NvHTTP unpinned = server.newClient("device", false, NativeApiFixtures.crypto());
            unpinned.getComputerDetails(METRICS_CAPS);
            NvHTTP noHttpsPort = new NvHTTP(new ComputerDetails.AddressTuple("127.0.0.1", server.httpPort()),
                    0, "device", server.serverCert(), NativeApiFixtures.crypto());
            noHttpsPort.getComputerDetails(METRICS_CAPS);
            for (NvHTTP unavailable : new NvHTTP[] {unpinned, noHttpsPort}) {
                assertThrows(IOException.class, unavailable::getHardwareMetrics);
            }
            assertEquals(1, server.requests().size());
        }
    }

    @Test
    public void networkPreservesTerminalAndTemporaryErrorsAndRejectsRedirects() throws Exception {
        AtomicReference<TestHttpServer.Response> reply = new AtomicReference<>();
        try (TestHttpServer server = new TestHttpServer(req -> reply.get())) {
            NvHTTP http = server.newClient("device", true, NativeApiFixtures.crypto());
            http.getComputerDetails(METRICS_CAPS);
            for (int code : new int[] {403, 409, 503, 302}) {
                reply.set(TestHttpServer.Response.json(code, "{\"error\":\"unavailable\"}")
                        .header("Location", "https://192.0.2.2:47984/other"));
                HostHttpResponseException error = assertThrows(HostHttpResponseException.class, http::getHardwareMetrics);
                assertEquals(code, error.getErrorCode());
            }
            reply.set(TestHttpServer.Response.text(404, "Not found"));
            assertThrows(FileNotFoundException.class, http::getHardwareMetrics);

            reply.set(TestHttpServer.Response.json(200, EMPTY.replace("\"version\":1", "\"version\":2")));
            assertThrows(HardwareMetrics.UnsupportedVersionException.class, http::getHardwareMetrics);
        }
    }

    @Test
    public void networkRejectsUnexpectedOrOversizedBodies() throws Exception {
        AtomicReference<TestHttpServer.Response> reply = new AtomicReference<>();
        try (TestHttpServer server = new TestHttpServer(req -> reply.get())) {
            NvHTTP http = server.newClient("device", true, NativeApiFixtures.crypto());
            http.getComputerDetails(METRICS_CAPS);
            reply.set(TestHttpServer.Response.bytes(200, "text/html", EMPTY.getBytes()));
            assertThrows(IOException.class, http::getHardwareMetrics);
            String oversized = String.join("", Collections.nCopies(HardwareMetrics.MAX_RESPONSE_BYTES + 1, " "));
            reply.set(TestHttpServer.Response.json(200, oversized));
            assertThrows(IOException.class, http::getHardwareMetrics);
        }
    }
}

package com.limelight.nvstream.http;

import android.app.Application;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.io.FileNotFoundException;
import java.io.IOException;
import java.lang.reflect.Field;
import java.security.KeyStore;
import java.security.PrivateKey;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.concurrent.TimeUnit;

import javax.net.ssl.TrustManager;
import javax.net.ssl.TrustManagerFactory;
import javax.net.ssl.X509TrustManager;

import okhttp3.Interceptor;
import okhttp3.MediaType;
import okhttp3.OkHttpClient;
import okhttp3.Protocol;
import okhttp3.Response;
import okhttp3.ResponseBody;

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
                "gpu_board_power_w", "gpu_vram_used_mb", "gpu_vram_usage_percent"};
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

    @Test
    public void networkUsesPinnedHttpsAndBoundedDeadline() throws Exception {
        NvHTTP http = http(true, 48001);
        intercept(http, chain -> {
            assertEquals("https", chain.request().url().scheme());
            assertEquals(48001, chain.request().url().port());
            assertEquals("/api/hardware-metrics", chain.request().url().encodedPath());
            assertEquals("device", chain.request().url().queryParameter("uniqueid"));
            assertEquals(TimeUnit.SECONDS.toNanos(2), chain.call().timeout().timeoutNanos());
            return response(chain, 200, "application/json; charset=utf-8", EMPTY);
        });
        assertEquals(1000, http.getHardwareMetrics().intervalMs);
        for (NvHTTP unavailable : new NvHTTP[] {http(false, 48001), http(true, 0)}) {
            intercept(unavailable, chain -> { throw new AssertionError("Unauthenticated discovery or request"); });
            assertThrows(IOException.class, unavailable::getHardwareMetrics);
        }
    }

    @Test
    public void networkPreservesTerminalAndTemporaryErrorsAndRejectsRedirects() throws Exception {
        NvHTTP http = http(true, 47984);
        for (int code : new int[] {403, 409, 503, 302}) {
            intercept(http, chain -> response(chain, code, "application/json", "{\"error\":\"unavailable\"}")
                    .newBuilder().header("Location", "https://192.0.2.2:47984/other").build());
            HostHttpResponseException error = assertThrows(HostHttpResponseException.class, http::getHardwareMetrics);
            assertEquals(code, error.getErrorCode());
        }
        intercept(http, chain -> response(chain, 404, "text/plain", "Not found"));
        assertThrows(FileNotFoundException.class, http::getHardwareMetrics);
        intercept(http, chain -> response(chain, 200, "application/json", EMPTY.replace("\"version\":1", "\"version\":2")));
        assertThrows(HardwareMetrics.UnsupportedVersionException.class, http::getHardwareMetrics);
    }

    @Test
    public void networkRejectsUnexpectedOrOversizedBodies() throws Exception {
        NvHTTP http = http(true, 47984);
        intercept(http, chain -> response(chain, 200, "text/html", EMPTY));
        assertThrows(IOException.class, http::getHardwareMetrics);
        String oversized = String.join("", Collections.nCopies(HardwareMetrics.MAX_RESPONSE_BYTES + 1, " "));
        intercept(http, chain -> response(chain, 200, "application/json", oversized));
        assertThrows(IOException.class, http::getHardwareMetrics);
    }

    private static Response response(Interceptor.Chain chain, int code, String type, String body) {
        return new Response.Builder().request(chain.request()).protocol(Protocol.HTTP_1_1)
                .code(code).message("Fixture").body(ResponseBody.create(body, MediaType.get(type))).build();
    }

    private static void intercept(NvHTTP http, Interceptor interceptor) throws Exception {
        Field client = NvHTTP.class.getDeclaredField("httpClientShortConnectTimeout");
        client.setAccessible(true);
        client.set(http, new OkHttpClient.Builder().addInterceptor(interceptor).build());
    }

    private static NvHTTP http(boolean pinned, int httpsPort) throws Exception {
        NvHTTP http = new NvHTTP(new ComputerDetails.AddressTuple("192.0.2.1", 47989), httpsPort,
                "device", null, new LimelightCryptoProvider() {
            public X509Certificate getClientCertificate() { return null; }
            public PrivateKey getClientPrivateKey() { return null; }
            public byte[] getPemEncodedClientCertificate() { return new byte[0]; }
            public String encodeBase64String(byte[] data) { return ""; }
        });
        if (pinned) {
            TrustManagerFactory factory = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm());
            factory.init((KeyStore) null);
            for (TrustManager manager : factory.getTrustManagers()) {
                if (manager instanceof X509TrustManager) {
                    http.setServerCert(((X509TrustManager) manager).getAcceptedIssuers()[0]);
                    break;
                }
            }
        }
        return http;
    }
}

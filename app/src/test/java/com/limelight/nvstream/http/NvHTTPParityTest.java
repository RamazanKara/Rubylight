package com.limelight.nvstream.http;

import com.limelight.nvstream.ConnectionContext;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.preferences.HostDisplayChoice;
import com.limelight.utils.ClipboardSync;

import org.junit.Test;

import java.io.InputStream;
import java.io.StringReader;
import java.nio.charset.StandardCharsets;
import java.security.PrivateKey;
import java.security.cert.X509Certificate;
import java.util.HashSet;
import java.util.LinkedList;
import java.util.Locale;
import java.util.Set;

import javax.crypto.spec.SecretKeySpec;

import okhttp3.HttpUrl;
import okhttp3.MediaType;
import okhttp3.OkHttpClient;
import okhttp3.Protocol;
import okhttp3.Response;
import okhttp3.ResponseBody;
import okio.Buffer;

import static org.junit.Assert.*;

public class NvHTTPParityTest {
    private NvHTTP http() throws java.io.IOException {
        return http("192.0.2.1", 47989);
    }

    private NvHTTP http(String address, int port) throws java.io.IOException {
        return new NvHTTP(new ComputerDetails.AddressTuple(address, port), 47984,
                "1234567890abcdef", null, new LimelightCryptoProvider() {
            public X509Certificate getClientCertificate() { return null; }
            public PrivateKey getClientPrivateKey() { return null; }
            public byte[] getPemEncodedClientCertificate() { return new byte[0]; }
            public String encodeBase64String(byte[] data) { return ""; }
        });
    }

    @Test
    public void cancellingAStreamAbortsAnIncompleteHttpBodyAndPreventsLaterRequests() throws Exception {
        java.util.concurrent.ExecutorService workers = java.util.concurrent.Executors.newFixedThreadPool(2);
        java.util.concurrent.CountDownLatch headersSent = new java.util.concurrent.CountDownLatch(1);
        java.util.concurrent.CountDownLatch finish = new java.util.concurrent.CountDownLatch(1);
        try (java.net.ServerSocket server = new java.net.ServerSocket(0, 1,
                java.net.InetAddress.getLoopbackAddress())) {
            NvHTTP http = http(server.getInetAddress().getHostAddress(), server.getLocalPort());
            java.util.concurrent.Future<?> host = workers.submit(() -> {
                try (java.net.Socket socket = server.accept()) {
                    java.io.BufferedReader reader = new java.io.BufferedReader(
                            new java.io.InputStreamReader(socket.getInputStream(), StandardCharsets.US_ASCII));
                    while (!reader.readLine().isEmpty()) { }
                    socket.getOutputStream().write(("HTTP/1.1 200 OK\r\nContent-Length: 100\r\n" +
                            "Content-Type: application/xml\r\n\r\n<root").getBytes(StandardCharsets.US_ASCII));
                    socket.getOutputStream().flush();
                    headersSent.countDown();
                    finish.await();
                } catch (Exception e) {
                    throw new RuntimeException(e);
                }
            });
            java.util.concurrent.Future<String> response = workers.submit(() -> http.getServerInfo(true));
            try {
                assertTrue(headersSent.await(5, java.util.concurrent.TimeUnit.SECONDS));
                http.cancelPendingRequests();
                java.util.concurrent.ExecutionException failure = assertThrows(
                        java.util.concurrent.ExecutionException.class,
                        () -> response.get(2, java.util.concurrent.TimeUnit.SECONDS));
                assertTrue(failure.getCause() instanceof java.io.IOException);
                assertThrows(java.io.IOException.class, () -> http.getServerInfo(true));
                assertThrows(java.io.IOException.class, http::probeServerLatency);
            } finally {
                finish.countDown();
            }
            host.get(2, java.util.concurrent.TimeUnit.SECONDS);
        } finally {
            finish.countDown();
            workers.shutdownNow();
        }
    }

    @Test
    public void latencyProbeHasABoundedDeadlineAndRejectsInvalidHostResponses() throws Exception {
        NvHTTP http = http();
        java.lang.reflect.Field client = NvHTTP.class.getDeclaredField("httpClientShortConnectTimeout");
        client.setAccessible(true);
        client.set(http, new OkHttpClient.Builder().addInterceptor(chain -> {
            assertEquals(java.util.concurrent.TimeUnit.SECONDS.toNanos(2), chain.call().timeout().timeoutNanos());
            assertEquals("/serverinfo", chain.request().url().encodedPath());
            return new Response.Builder().request(chain.request()).protocol(Protocol.HTTP_1_1)
                    .code(200).message("OK").body(ResponseBody.create(
                            "<root status_code=\"200\"><appversion>7.1.0.0</appversion></root>",
                            MediaType.get("application/xml"))).build();
        }).build());
        assertTrue(http.probeServerLatency() >= 0);
        client.set(http, new OkHttpClient.Builder().addInterceptor(chain ->
                new Response.Builder().request(chain.request()).protocol(Protocol.HTTP_1_1)
                        .code(200).message("OK").body(ResponseBody.create("<root status_code=\"200\" />",
                                MediaType.get("application/xml"))).build()).build());
        assertThrows(org.xmlpull.v1.XmlPullParserException.class, http::probeServerLatency);
    }

    private ComputerDetails capabilities(String xml) throws Exception {
        ComputerDetails details = new ComputerDetails();
        NvHTTP.readClientCapabilities(details, xml);
        return details;
    }

    @Test
    public void hardwareMetricsRequireSupportedDiscoveryAndPairedViewingPermission() throws Exception {
        ComputerDetails details = capabilities("<root status_code=\"200\"><RustHostVersion>2.2.5</RustHostVersion>" +
                "<PairStatus>1</PairStatus><HardwareMetricsVersion>1</HardwareMetricsVersion><Permission>" +
                ComputerDetails.PERMISSION_VIEW + "</Permission></root>");
        assertFalse(details.supportsHardwareMetrics());
        details.pairState = PairingManager.PairState.PAIRED;
        assertTrue(details.supportsHardwareMetrics());
        ComputerDetails copy = new ComputerDetails(details);
        assertTrue(copy.supportsHardwareMetrics());
        copy.permission = ComputerDetails.PERMISSION_INPUT;
        assertFalse(copy.supportsHardwareMetrics());
        copy.permission = ComputerDetails.PERMISSION_LAUNCH;
        assertTrue(copy.supportsHardwareMetrics());
        // The field decides, not the host's brand: without it there are no metrics.
        copy.caps = HostCapabilities.fromServerInfo("<root status_code=\"200\"><RustHostVersion>2.2.5</RustHostVersion>" +
                "<PairStatus>1</PairStatus></root>");
        assertFalse(copy.supportsHardwareMetrics());
        for (String version : new String[] {"0", "2", "-1", "invalid", ""}) {
            assertFalse(version, HostCapabilities.fromServerInfo("<root status_code=\"200\"><PairStatus>1</PairStatus>" +
                    "<RustHostVersion>2.2.5</RustHostVersion><HardwareMetricsVersion>" + version +
                    "</HardwareMetricsVersion></root>").has(HostCapabilities.Feature.HARDWARE_METRICS));
        }
        NvHTTP.readClientCapabilities(details, "<root status_code=\"200\" />");
        assertFalse(details.supportsHardwareMetrics());
        copy.update(details);
        assertFalse(copy.caps.has(HostCapabilities.Feature.HARDWARE_METRICS));
        assertFalse(copy.supportsHardwareMetrics());
    }

    @Test
    public void encoderBitratesAreCopiedAndMissingOrInvalidRatesStayUnknown() throws Exception {
        ComputerDetails details = capabilities("<root status_code=\"200\">" +
                "<RustHostEncoderBitrateKbps>30000</RustHostEncoderBitrateKbps>" +
                "<RustHostInitialEncoderBitrateKbps>38988</RustHostInitialEncoderBitrateKbps></root>");
        ComputerDetails copy = new ComputerDetails(details);
        assertTrue(copy.encoderBitrateSupported);
        assertEquals(30000, copy.encoderBitrateKbps);
        assertEquals(38988, copy.initialEncoderBitrateKbps);
        for (String value : new String[] {"", "0", "-1", "2000001", "invalid"}) {
            NvHTTP.readClientCapabilities(details, "<root status_code=\"200\">" +
                    "<RustHostEncoderBitrateKbps>" + value + "</RustHostEncoderBitrateKbps>" +
                    "<RustHostInitialEncoderBitrateKbps>" + value + "</RustHostInitialEncoderBitrateKbps></root>");
            assertTrue(details.encoderBitrateSupported);
            assertEquals(0, details.encoderBitrateKbps);
            assertEquals(0, details.initialEncoderBitrateKbps);
        }
        NvHTTP.readClientCapabilities(copy, "<root status_code=\"200\" />");
        assertFalse(copy.encoderBitrateSupported);
        assertEquals(0, copy.encoderBitrateKbps);
        assertEquals(0, copy.initialEncoderBitrateKbps);
        details.update(copy);
        assertFalse(details.encoderBitrateSupported);
        for (String field : new String[] {"RustHostEncoderBitrateKbps", "RustHostInitialEncoderBitrateKbps"}) {
            assertFalse(capabilities("<root status_code=\"200\"><" + field + ">0</" + field + "></root>")
                    .encoderBitrateSupported);
        }
    }

    @Test
    public void rubylightAdvertisesUnsignedPermissionsAndFractionalLimiter() throws Exception {
        ComputerDetails details = http().getComputerDetails(fixture("butterpollo-serverinfo.xml"));
        assertEquals(0xFFFFFFFFL, details.permission);
        assertTrue(details.canReadClipboard());
        assertTrue(details.canWriteClipboard());
        assertTrue(details.caps.has(HostCapabilities.Feature.FRAME_LIMITER_STATUS));
        assertTrue(details.frameLimiterEnabled);
        assertTrue(details.virtualDisplayFrameLimiterEnabled);
        assertEquals(59940, details.frameLimiterFpsLimitMilliHz);
        assertArrayEquals(new byte[] {0}, details.serverCommandPayload(0));
    }

    @Test
    public void apolloCommandsPreserveEmptyNamesIndicesAndReservedBytes() throws Exception {
        ComputerDetails details = capabilities(fixture("apollo-serverinfo.xml"));
        assertEquals("Display & audio", details.serverCommands.get(0));
        assertEquals("", details.serverCommands.get(1));
        assertEquals("Sleep", details.serverCommands.get(2));
        assertArrayEquals(new byte[] {2, 0, 0, 0}, details.serverCommandPayload(2));
        assertFalse(details.canRunServerCommand(-1));
        assertFalse(details.canRunServerCommand(3));
        assertThrows(IllegalArgumentException.class, () -> details.serverCommandPayload(3));
    }

    @Test
    public void commandIndexCannotWrapBeyondOneByte() {
        ComputerDetails details = new ComputerDetails();
        for (int i = 0; i < 257; i++) {
            details.serverCommands.add("Command " + i);
        }
        assertArrayEquals(new byte[] {(byte)255, 0, 0, 0}, details.serverCommandPayload(255));
        assertFalse(details.canRunServerCommand(256));
    }

    @Test
    public void viewerCanResumeButCannotLaunchQuitInputOrUseHostActions() throws Exception {
        ComputerDetails details = http().getComputerDetails(fixture("butterpollo-viewer-serverinfo.xml"));
        assertTrue(details.hasPermission(ComputerDetails.PERMISSION_LIST));
        assertTrue(details.hasPermission(ComputerDetails.PERMISSION_VIEW | ComputerDetails.PERMISSION_LAUNCH));
        assertFalse(details.hasPermission(ComputerDetails.PERMISSION_LAUNCH));
        assertFalse(details.hasPermission(ComputerDetails.PERMISSION_INPUT));
        assertFalse(details.canReadClipboard());
        assertFalse(details.canWriteClipboard());
        assertFalse(details.canRunServerCommand(0));
        assertFalse(details.frameLimiterEnabled);
        assertTrue(details.virtualDisplayFrameLimiterEnabled);
        assertEquals(0, details.frameLimiterFpsLimitMilliHz);
    }

    @Test
    public void clipboardDirectionsAreIndependentAndRequireViewing() throws Exception {
        ComputerDetails details = capabilities("<root status_code=\"200\"><Permission>" +
                (ComputerDetails.PERMISSION_LAUNCH | ComputerDetails.PERMISSION_CLIPBOARD_SET) + "</Permission></root>");
        assertTrue(details.canWriteClipboard());
        assertFalse(details.canReadClipboard());
        details.permission = ComputerDetails.PERMISSION_VIEW | ComputerDetails.PERMISSION_CLIPBOARD_READ;
        assertTrue(details.canReadClipboard());
        assertFalse(details.canWriteClipboard());
        details.permission = ComputerDetails.PERMISSION_CLIPBOARD_READ | ComputerDetails.PERMISSION_CLIPBOARD_SET;
        assertFalse(details.canReadClipboard());
        assertFalse(details.canWriteClipboard());
    }

    @Test
    public void stockSunshineKeepsNormalActionsWithoutApolloExtras() throws Exception {
        ComputerDetails details = capabilities(fixture("sunshine-serverinfo.xml"));
        assertEquals(-1, details.permission);
        assertTrue(details.hasPermission(ComputerDetails.PERMISSION_LAUNCH));
        assertTrue(details.hasPermission(ComputerDetails.PERMISSION_VIEW));
        assertFalse(details.canReadClipboard());
        assertFalse(details.canWriteClipboard());
        assertFalse(details.canRunServerCommand(0));
        assertFalse(details.caps.has(HostCapabilities.Feature.FRAME_LIMITER_STATUS));
    }

    @Test
    public void malformedPermissionsFailClosedWithoutBreakingDiscovery() throws Exception {
        for (String value : new String[] {"", "-1", "4294967296", "not-a-mask"}) {
            ComputerDetails details = capabilities("<root status_code=\"200\"><Permission>" + value +
                    "</Permission><ServerCommand>Sleep</ServerCommand></root>");
            assertEquals(0, details.permission);
            assertFalse(details.canRunServerCommand(0));
            assertFalse(details.hasPermission(ComputerDetails.PERMISSION_LAUNCH));
        }
    }

    @Test
    public void malformedHostNumbersUseCheckedErrorsOrOptionalFieldDefaults() throws Exception {
        NvHTTP http = http();
        for (String status : new String[] {"", "garbage", "4294967496", "-4294967096"}) {
            String xml = "<root status_code=\"" + status + "\"><appversion>7.1.2.3</appversion></root>";
            // Garbage, not a host error: the host gave no code or reason to show as its own.
            assertThrows(org.xmlpull.v1.XmlPullParserException.class, () -> http.getServerVersion(xml));
        }
        for (String value : new String[] {"-1", "65536", "garbage"}) {
            String xml = "<root status_code=\"200\"><HttpsPort>" + value +
                    "</HttpsPort><ExternalPort>" + value + "</ExternalPort></root>";
            assertEquals(47984, http.getHttpsPort(xml));
            assertEquals(47989, http.getExternalPort(xml));
        }
        for (String value : new String[] {"-1", "4294967296", "garbage"}) {
            assertEquals(0, http.getServerCodecModeSupport("<root status_code=\"200\"><ServerCodecModeSupport>" +
                    value + "</ServerCodecModeSupport></root>"));
        }
        assertThrows(org.xmlpull.v1.XmlPullParserException.class, () -> http.getCurrentGame(
                "<root status_code=\"200\"><state>SUNSHINE_SERVER_BUSY</state><currentgame>bad</currentgame></root>"));
        for (String version : new String[] {"7.1", "7.x.0.0", "7.1.-1.0", "7.1.431.-2"}) {
            assertThrows(org.xmlpull.v1.XmlPullParserException.class, () -> http.getServerAppVersionQuad(
                    "<root status_code=\"200\"><appversion>" + version + "</appversion></root>"));
        }
        assertArrayEquals(new int[] {7, 1, 431, -1}, http.getServerAppVersionQuad(
                "<root status_code=\"200\"><appversion>7.1.431.-1</appversion></root>"));
    }

    @Test
    public void stockUnsignedErrorStatusKeepsItsAudioDiagnostic() {
        HostHttpResponseException error = assertThrows(HostHttpResponseException.class,
                () -> NvHTTP.getXmlString("<root status_code=\"4294967295\" status_message=\"Invalid\"/>",
                        "appversion", true));
        assertEquals(418, error.getErrorCode());
    }

    @Test
    public void capabilityRefreshClearsOldGrantsAndDoesNotAliasCommands() throws Exception {
        ComputerDetails original = capabilities(fixture("butterpollo-serverinfo.xml"));
        ComputerDetails copy = new ComputerDetails(original);
        original.serverCommands.clear();
        assertTrue(copy.canRunServerCommand(0));
        copy.update(http().getComputerDetails(fixture("butterpollo-viewer-serverinfo.xml")));
        assertFalse(copy.canRunServerCommand(0));
        assertFalse(copy.canWriteClipboard());
        NvHTTP.readClientCapabilities(copy, fixture("sunshine-serverinfo.xml"));
        assertEquals(-1, copy.permission);
        assertEquals(HostCapabilities.Family.SUNSHINE, copy.caps.family());
        assertFalse(copy.caps.has(HostCapabilities.Feature.FRAME_LIMITER_STATUS));
    }

    @Test
    public void clipboardUsesHttpsPathPostBodyAndPerRequestIdentity() throws Exception {
        NvHTTP http = http();
        String text = "Grüße & <text>\nこんにちは 🧈";
        OkHttpClient client = new OkHttpClient.Builder().addInterceptor(chain -> {
            assertEquals("https", chain.request().url().scheme());
            assertEquals("/actions/clipboard", chain.request().url().encodedPath());
            assertEquals("text", chain.request().url().queryParameter("type"));
            assertEquals("1234567890abcdef", chain.request().url().queryParameter("uniqueid"));
            assertNotNull(chain.request().url().queryParameter("uuid"));
            assertFalse(chain.request().url().toString().contains("Grüße"));
            if (chain.request().method().equals("POST")) {
                Buffer buffer = new Buffer();
                chain.request().body().writeTo(buffer);
                assertEquals(text, buffer.readUtf8());
                assertEquals("text/plain; charset=utf-8", chain.request().body().contentType().toString());
            } else {
                assertEquals("GET", chain.request().method());
            }
            return new Response.Builder().request(chain.request()).protocol(Protocol.HTTP_1_1)
                    .code(200).message("OK").body(ResponseBody.create(
                            chain.request().method().equals("POST") ? "" : text,
                            MediaType.get("text/plain; charset=utf-8"))).build();
        }).build();
        java.lang.reflect.Field field = NvHTTP.class.getDeclaredField("httpClientLongConnectTimeout");
        field.setAccessible(true);
        field.set(http, client);
        http.sendClipboard(text);
        assertEquals(text, http.getClipboard());
    }

    @Test
    public void clipboardRejectsXmlErrorPagesAndOversizedTransfersButAcceptsEmptyText() throws Exception {
        try (ResponseBody empty = ResponseBody.create("", MediaType.get("text/plain"));
             ResponseBody error = ResponseBody.create("<root status_code=\"404\"/>", MediaType.get("application/xml"));
             ResponseBody large = ResponseBody.create("a".repeat(1024 * 1024 + 1), MediaType.get("text/plain"))) {
            assertEquals("", NvHTTP.readClipboardResponse(empty));
            assertThrows(java.io.IOException.class, () -> NvHTTP.readClipboardResponse(error));
            assertThrows(java.io.IOException.class, () -> NvHTTP.readClipboardResponse(large));
        }
        assertEquals(1024 * 1024, NvHTTP.clipboardRequestBody("a".repeat(1024 * 1024)).contentLength());
        assertThrows(java.io.IOException.class, () -> NvHTTP.clipboardRequestBody("ä".repeat(524289)));
    }

    @Test
    public void bandwidthProbeRequiresPairingAndBoundedAdvertisedCapability() throws Exception {
        ComputerDetails details = new ComputerDetails();
        for (String bytes : new String[] {"0", "-1", "33554433", "bad", "33554432"}) {
            NvHTTP.readClientCapabilities(details, "<root status_code=\"200\"><RustHostVersion>test</RustHostVersion>" +
                    "<PairStatus>1</PairStatus><PyroWaveHostLinkMbps>2500</PyroWaveHostLinkMbps><PyroWaveBandwidthProbeBytes>" +
                    bytes + "</PyroWaveBandwidthProbeBytes></root>");
            details.pairState = PairingManager.PairState.NOT_PAIRED;
            assertFalse(details.supportsPyroWaveBandwidthProbe());
            details.pairState = PairingManager.PairState.PAIRED;
            assertEquals(bytes.equals("33554432"), details.supportsPyroWaveBandwidthProbe());
            assertEquals(2500, new ComputerDetails(details).pyroWaveHostLinkMbps);
        }
        NvHTTP client = http();
        assertThrows(java.io.IOException.class, () -> client.probePyroWaveBandwidth(details, percent -> {}));
        NvHTTP.readClientCapabilities(details, "<root status_code=\"200\"/>");
        assertFalse(details.supportsPyroWaveBandwidthProbe());
        assertEquals(0, details.pyroWaveHostLinkMbps);
    }

    @Test
    public void bandwidthProbeStreamsBoundedChunksAndReportsProgress() throws Exception {
        java.util.List<Integer> progress = new java.util.ArrayList<>();
        try (ResponseBody response = ResponseBody.create(new byte[128 * 1024], MediaType.get("application/octet-stream"))) {
            NvHTTP.readBandwidthProbe(response, 128 * 1024, progress::add);
        }
        assertFalse(progress.isEmpty());
        assertEquals(Integer.valueOf(100), progress.get(progress.size() - 1));
        int previous = -1;
        for (int percent : progress) {
            assertTrue(percent > previous && percent >= 0 && percent <= 100);
            previous = percent;
        }
    }

    @Test
    public void bandwidthProbeRejectsErrorPagesAndAdvertisedLengthMismatch() throws Exception {
        try (ResponseBody error = ResponseBody.create("error", MediaType.get("application/xml"));
             ResponseBody wrongLength = ResponseBody.create(new byte[20], MediaType.get("application/octet-stream"))) {
            assertThrows(java.io.IOException.class, () -> NvHTTP.readBandwidthProbe(error, 5, percent -> {}));
            assertThrows(java.io.IOException.class, () -> NvHTTP.readBandwidthProbe(wrongLength, 10, percent -> {}));
            assertThrows(java.io.IOException.class, () -> NvHTTP.readBandwidthProbe(wrongLength, 33554433, percent -> {}));
        }
    }

    @Test
    public void bandwidthProbeRejectsTruncatedAndOversizedChunkedBodies() throws Exception {
        for (int length : new int[] {9, 10, 11}) {
            Buffer source = new Buffer().write(new byte[length]);
            try (ResponseBody response = new ResponseBody() {
                @Override public MediaType contentType() { return MediaType.get("application/octet-stream"); }
                @Override public long contentLength() { return -1; }
                @Override public okio.BufferedSource source() { return source; }
            }) {
                if (length == 10) {
                    NvHTTP.readBandwidthProbe(response, 10, percent -> {});
                } else {
                    assertThrows(java.io.IOException.class, () -> NvHTTP.readBandwidthProbe(response, 10, percent -> {}));
                }
            }
        }
    }

    @Test
    public void runtimeBitrateUsesKbpsAndReportsTheHostAppliedCap() throws Exception {
        NvHTTP http = http();
        OkHttpClient client = new OkHttpClient.Builder().addInterceptor(chain -> {
            assertEquals("https", chain.request().url().scheme());
            assertEquals("/bitrate", chain.request().url().encodedPath());
            assertEquals("30000", chain.request().url().queryParameter("bitrate"));
            return new Response.Builder().request(chain.request()).protocol(Protocol.HTTP_1_1)
                    .code(200).message("OK").body(ResponseBody.create(
                            "<root status_code=\"200\"><bitrate>20000</bitrate><updated>1</updated></root>",
                            MediaType.get("application/xml"))).build();
        }).build();
        java.lang.reflect.Field field = NvHTTP.class.getDeclaredField("httpClientLongConnectTimeout");
        field.setAccessible(true);
        field.set(http, client);
        assertEquals(20000, http.setBitrate(30000));
        assertThrows(IllegalArgumentException.class, () -> http.setBitrate(0));
        assertThrows(IllegalArgumentException.class, () -> http.setBitrate(2000001));
    }

    @Test
    public void hostActionsPropagateRevocationUnsupportedEndpointsAndInvalidSuccessBodies() throws Exception {
        NvHTTP http = http();
        java.lang.reflect.Field field = NvHTTP.class.getDeclaredField("httpClientLongConnectTimeout");
        field.setAccessible(true);
        for (int code : new int[] {403, 404, 500, 200}) {
            field.set(http, new OkHttpClient.Builder().addInterceptor(chain ->
                    new Response.Builder().request(chain.request()).protocol(Protocol.HTTP_1_1)
                            .code(code).message("Fixture response").body(ResponseBody.create(
                                    "<root status_code=\"200\"><bitrate>0</bitrate></root>",
                                    MediaType.get("application/xml"))).build()).build());
            assertThrows(java.io.IOException.class, http::getClipboard);
            assertThrows(java.io.IOException.class, () -> http.sendClipboard("text"));
            assertThrows(java.io.IOException.class, () -> http.setBitrate(10000));
        }
    }

    @Test
    public void requestNonceDoesNotReplaceThePersistentDeviceIdentity() throws Exception {
        LimelightCryptoProvider crypto = new LimelightCryptoProvider() {
            public X509Certificate getClientCertificate() { return null; }
            public PrivateKey getClientPrivateKey() { return null; }
            public byte[] getPemEncodedClientCertificate() { return new byte[0]; }
            public String encodeBase64String(byte[] data) { return ""; }
        };
        NvHTTP first = new NvHTTP(new ComputerDetails.AddressTuple("192.0.2.1", 47989),
                47984, "1234567890abcdef", null, crypto);
        NvHTTP second = new NvHTTP(new ComputerDetails.AddressTuple("192.0.2.1", 47989),
                47984, "fedcba0987654321", null, crypto);
        HttpUrl base = HttpUrl.get("https://192.0.2.1:47984");
        HttpUrl one = first.getCompleteUrl(base, "serverinfo", null);
        HttpUrl again = first.getCompleteUrl(base, "resume", "appid=42");
        assertEquals("1234567890abcdef", one.queryParameter("uniqueid"));
        assertEquals(one.queryParameter("uniqueid"), again.queryParameter("uniqueid"));
        assertNotEquals(one.queryParameter("uuid"), again.queryParameter("uuid"));
        assertNotEquals(one.queryParameter("uniqueid"), second.getCompleteUrl(base, "serverinfo", null).queryParameter("uniqueid"));
    }

    @Test
    public void failedPairingUsesHttpBeforeRegistrationWhilePairedUnpairUsesHttps() throws Exception {
        NvHTTP http = http();
        javax.net.ssl.TrustManagerFactory factory = javax.net.ssl.TrustManagerFactory.getInstance(
                javax.net.ssl.TrustManagerFactory.getDefaultAlgorithm());
        factory.init((java.security.KeyStore) null);
        for (javax.net.ssl.TrustManager manager : factory.getTrustManagers()) {
            if (manager instanceof javax.net.ssl.X509TrustManager) {
                http.setServerCert(((javax.net.ssl.X509TrustManager) manager).getAcceptedIssuers()[0]);
                break;
            }
        }
        java.util.List<String> schemes = new java.util.ArrayList<>();
        java.lang.reflect.Field field = NvHTTP.class.getDeclaredField("httpClientLongConnectTimeout");
        field.setAccessible(true);
        field.set(http, new OkHttpClient.Builder().addInterceptor(chain -> {
            schemes.add(chain.request().url().scheme());
            assertEquals("/unpair", chain.request().url().encodedPath());
            assertEquals("1234567890abcdef", chain.request().url().queryParameter("uniqueid"));
            return new Response.Builder().request(chain.request()).protocol(Protocol.HTTP_1_1)
                    .code(200).message("OK").body(ResponseBody.create(
                            "<root status_code=\"200\"><unpaired>1</unpaired></root>",
                            MediaType.get("application/xml"))).build();
        }).build());
        http.cancelPairing();
        http.unpair();
        assertEquals(java.util.Arrays.asList("http", "https"), schemes);
    }

    @Test
    public void oneTimePinHashUsesPaddedPinUppercaseSaltTextAndUtf8Passphrase() throws Exception {
        byte[] salt = new byte[] {1, 35, 69, 103, (byte) 137, (byte) 171, (byte) 205, (byte) 239,
                1, 35, 69, 103, (byte) 137, (byte) 171, (byte) 205, (byte) 239};
        assertEquals("083CF8E1505EBE84ECC0288156E3FCECBB31FE9650F64A6DA0908C3270B8DE4B",
                PairingManager.oneTimePinAuth("0042", salt, "Grüße 🧈"));
        assertNotEquals(PairingManager.oneTimePinAuth("0042", salt, "phrase"),
                PairingManager.oneTimePinAuth("0042", salt, "phrase "));
    }

    @Test
    public void oneTimePinPairingAddsOnlyAuthHashAndUsesReadTimeout() throws Exception {
        LimelightCryptoProvider crypto = new LimelightCryptoProvider() {
            public X509Certificate getClientCertificate() { return null; }
            public PrivateKey getClientPrivateKey() { return null; }
            public byte[] getPemEncodedClientCertificate() { return new byte[0]; }
            public String encodeBase64String(byte[] data) { return ""; }
        };
        for (String passphrase : new String[] {null, "one-time secret"}) {
            NvHTTP http = new NvHTTP(new ComputerDetails.AddressTuple("192.0.2.1", 47989),
                    47984, "test-device", null, crypto) {
                @Override
                String executePairingCommand(String arguments, boolean readTimeout) {
                    HttpUrl query = HttpUrl.get("http://host/pair?" + arguments);
                    assertEquals("getservercert", query.queryParameter("phrase"));
                    assertEquals(passphrase != null, readTimeout);
                    if (passphrase == null) {
                        assertNull(query.queryParameter("otpauth"));
                    } else {
                        assertTrue(query.queryParameter("otpauth").matches("[0-9A-F]{64}"));
                        assertFalse(arguments.contains(passphrase));
                        assertNull(query.queryParameter("pin"));
                    }
                    return "<root status_code=\"200\"><paired>0</paired></root>";
                }
            };
            assertEquals(PairingManager.PairState.FAILED, http.getPairingManager().pair(
                    "<root status_code=\"200\"><appversion>7.1.2.3</appversion></root>", "0042", passphrase));
        }
    }

    @Test
    public void pyrowaveServerCapabilitiesRemainSeparateFromLaunchHdr() throws Exception {
        ConnectionContext context = context("butterpollo-serverinfo.xml");
        String server = fixture("butterpollo-serverinfo.xml");
        assertEquals(0x07830301L, Long.parseLong(NvHTTP.getXmlString(server, "ServerCodecModeSupport", true)));
        assertNull(query(context, false).queryParameter("hdrMode"));
        assertEquals("1", query(context, true).queryParameter("hdrMode"));
        assertFalse(query(context, true).toString().toLowerCase(Locale.ROOT).contains("pyrowave"));
    }

    private String fixture(String name) throws Exception {
        try (InputStream input = getClass().getResourceAsStream("/protocol/" + name)) {
            assertNotNull(input);
            return new String(input.readAllBytes(), StandardCharsets.UTF_8);
        }
    }

    private ConnectionContext context(String server) throws Exception {
        ConnectionContext context = new ConnectionContext();
        context.streamConfig = new StreamConfiguration.Builder()
                .setResolution(1968, 2184).setRefreshRate(120).setLaunchRefreshRate(120)
                .setClientRefreshRateX100(11988).setVirtualDisplay(true, 100).build();
        context.negotiatedWidth = 1968;
        context.negotiatedHeight = 2184;
        context.riKey = new SecretKeySpec(new byte[16], "AES");
        context.riKeyId = -123;
        context.hostCaps = TestHosts.fixture(server);
        return context;
    }

    private HttpUrl query(ConnectionContext context, boolean hdr) {
        return HttpUrl.get("https://host/launch?" + NvHTTP.getLaunchQuery(context, 42, hdr));
    }

    @Test
    public void rubylightLaunchPreservesNativePixelsAndFractionalHighRefresh() throws Exception {
        ConnectionContext context = context("butterpollo-serverinfo.xml");
        Locale previous = Locale.getDefault();
        try {
            Locale.setDefault(Locale.GERMANY);
            HttpUrl query = query(context, true);
            assertEquals("1968x2184x119.88", query.queryParameter("mode"));
            assertEquals("1", query.queryParameter("virtualDisplay"));
            assertEquals("100", query.queryParameter("scaleFactor"));
            assertEquals("1", query.queryParameter("hdrMode"));
            assertEquals("42", query.queryParameter("appid"));
            assertEquals("-123", query.queryParameter("rikeyid"));
        } finally {
            Locale.setDefault(previous);
        }
    }

    @Test
    public void tenBitSdrSaysHdrModeZeroAndOtherStreamsKeepTheirQuery() throws Exception {
        ConnectionContext context = context("butterpollo-serverinfo.xml");
        // Plain 8-bit and HDR are as before.
        assertNull(query(context, false).queryParameter("hdrMode"));
        assertEquals("1", query(context, true).queryParameter("hdrMode"));
        // 10-bit SDR that the PC accepted says 0 outright, on launch and resume alike.
        context.streamConfig = new StreamConfiguration.Builder().setLaunchRefreshRate(60).setSdr10Bit(true).build();
        context.negotiated10Bit = true;
        assertEquals("0", query(context, false).queryParameter("hdrMode"));
        // Real HDR is not weakened by the setting.
        assertEquals("1", query(context, true).queryParameter("hdrMode"));
        // A PC that landed on 8-bit is a plain 8-bit stream.
        context.negotiated10Bit = false;
        assertNull(query(context, false).queryParameter("hdrMode"));
    }

    @Test
    public void theFecChoiceGoesOnlyToAHostThatTakesIt() throws Exception {
        assertFalse(context("butterpollo-serverinfo.xml").hostCaps.has(HostCapabilities.Feature.CLIENT_FEC));
        assertTrue(context("rubylight-30-serverinfo.xml").hostCaps.has(HostCapabilities.Feature.CLIENT_FEC));
        for (String server : new String[] {"butterpollo-serverinfo.xml", "rubylight-30-serverinfo.xml"}) {
            ConnectionContext context = context(server);
            String plain = NvHTTP.getLaunchQuery(context, 42, false);
            assertFalse(server, plain.contains(FecChoice.LAUNCH_PARAMETER));
            context.fecChoice = FecChoice.AUTO;
            String asked = NvHTTP.getLaunchQuery(context, 42, false);
            if (!context.hostCaps.has(HostCapabilities.Feature.CLIENT_FEC)) {
                // A host without FecControlVersion gets exactly the query it got before.
                assertEquals(server, plain, asked);
                continue;
            }
            // The rest of the query stays byte for byte; the choice comes last, before the core's corever.
            assertEquals(plain + "&rlFecPercentage=auto", asked);
            context.fecChoice = FecChoice.HOST;
            assertEquals("host", query(context, false).queryParameter("rlFecPercentage"));
            context.fecChoice = FecChoice.fixed(255);
            assertEquals("255", query(context, false).queryParameter("rlFecPercentage"));
        }
        assertThrows(IllegalArgumentException.class, () -> FecChoice.fixed(256));
        assertThrows(IllegalArgumentException.class, () -> FecChoice.fixed(-1));
    }

    @Test
    public void rateChangesAreAskedForOnlyWhenTheStreamCanSendThem() throws Exception {
        for (String server : new String[] {"butterpollo-serverinfo.xml", "rubylight-30-serverinfo.xml"}) {
            ConnectionContext context = context(server);
            String plain = NvHTTP.getLaunchQuery(context, 42, false);
            assertFalse(server, plain.contains(NvHTTP.RATE_CHANGE_LAUNCH_PARAMETER));
            context.rateChange = true;
            // Any host gets it, last, and the rest of the query stays byte for byte.
            assertEquals(server, plain + "&rlRateChange=1", NvHTTP.getLaunchQuery(context, 42, false));
        }
        assertTrue(new StreamConfiguration.Builder().setRateChange(true).build().getRateChange());
        assertFalse(new StreamConfiguration.Builder().build().getRateChange());
    }

    @Test
    public void displayLuminanceGoesToOtherHostsAtLaunchAndToRubylightByControlMessage() throws Exception {
        com.limelight.utils.HdrBrightness luminance =
                com.limelight.utils.HdrBrightness.create(1000.125f, 0.0004f, 600.25f);
        for (String server : new String[] {"sunshine-serverinfo.xml", "vibepollo-serverinfo.xml",
                "apollo-serverinfo.xml", "butterpollo-serverinfo.xml", "rubylight-30-serverinfo.xml"}) {
            ConnectionContext context = context(server);
            // Unknown luminance sends nothing, so the host picks its own fallback.
            assertNull(server, query(context, true).queryParameter("maxBrightness"));
            context.streamConfig = new StreamConfiguration.Builder()
                    .setResolution(1968, 2184).setRefreshRate(120).setLaunchRefreshRate(120)
                    .setClientRefreshRateX100(11988).setVirtualDisplay(true, 100)
                    .setDisplayLuminance(luminance).build();
            HttpUrl query = query(context, true);
            if (context.hostCaps.family() == HostCapabilities.Family.RUBYLIGHT) {
                assertNull(server, query.queryParameter("maxBrightness"));
                continue;
            }
            // Float precision both ways, whatever the locale.
            assertEquals(server, 1000.125f, Float.parseFloat(query.queryParameter("maxBrightness")), 0f);
            assertEquals(server, 0.0004f, Float.parseFloat(query.queryParameter("minBrightness")), 0f);
            assertEquals(server, 600.25f, Float.parseFloat(query.queryParameter("maxAverageBrightness")), 0f);
        }
    }

    @Test
    public void onlyARubylightHostHearsWhetherTheLaunchConfirmsTheReplace() throws Exception {
        for (String server : new String[] {"butterpollo-serverinfo.xml", "rubylight-30-serverinfo.xml"}) {
            ConnectionContext context = context(server);
            assertEquals(server, HostCapabilities.Family.RUBYLIGHT, context.hostCaps.family());
            assertEquals(server, "0", query(context, false).queryParameter(LaunchConfirmation.REPLACE_PARAMETER));
            context.confirmReplace = true;
            assertEquals(server, "1", query(context, false).queryParameter(LaunchConfirmation.REPLACE_PARAMETER));
        }
        // Vibepollo and the rest keep their 60-second relaunch rule and get the query they got before.
        for (String server : new String[] {"vibepollo-serverinfo.xml", "apollo-serverinfo.xml",
                "sunshine-serverinfo.xml", "gamestream-serverinfo.xml"}) {
            ConnectionContext context = context(server);
            assertNotEquals(server, HostCapabilities.Family.RUBYLIGHT, context.hostCaps.family());
            String plain = NvHTTP.getLaunchQuery(context, 42, false);
            context.confirmReplace = true;
            assertEquals(server, plain, NvHTTP.getLaunchQuery(context, 42, false));
            assertFalse(server, plain.contains(LaunchConfirmation.REPLACE_PARAMETER));
        }
    }

    // Clipboard sync (0x5508) is asked only of a Rubylight PC; any other host gets the query it got before.
    @Test
    public void clipboardSyncIsAskedOnlyOfARubylightPc() throws Exception {
        Set<HostCapabilities.Family> seen = new HashSet<>();
        for (String server : new String[] {"butterpollo-serverinfo.xml", "rubylight-30-serverinfo.xml",
                "rubylight-serverinfo.xml", "sunshine-serverinfo.xml", "apollo-serverinfo.xml",
                "vibepollo-serverinfo.xml", "gamestream-serverinfo.xml"}) {
            ConnectionContext context = context(server);
            String plain = NvHTTP.getLaunchQuery(context, 42, false);
            assertFalse(server, plain.contains(ClipboardSync.LAUNCH_PARAMETER));
            context.streamConfig = new StreamConfiguration.Builder()
                    .setResolution(1968, 2184).setRefreshRate(120).setLaunchRefreshRate(120)
                    .setClientRefreshRateX100(11988).setVirtualDisplay(true, 100).setClipboardSync(true).build();
            String asked = NvHTTP.getLaunchQuery(context, 42, false);
            seen.add(context.hostCaps.family());
            if (context.hostCaps.family() != HostCapabilities.Family.RUBYLIGHT) {
                assertFalse(server, NvHTTP.asksClipboardSync(context));
                assertEquals(server, plain, asked);
                continue;
            }
            assertTrue(server, NvHTTP.asksClipboardSync(context));
            // The rest of the query stays byte for byte; the ask comes last.
            assertEquals(server, plain + "&" + ClipboardSync.LAUNCH_PARAMETER + "=1", asked);
        }
        assertTrue(seen.contains(HostCapabilities.Family.RUBYLIGHT));
        assertTrue(seen.size() > 1);
    }

    // The Error correction setting stores the wire value; anything else leaves it to the PC.
    @Test
    public void theFecSettingReadsBackAsItsChoice() {
        assertEquals(FecChoice.HOST, FecChoice.fromPreference("host"));
        assertEquals(FecChoice.AUTO, FecChoice.fromPreference("auto"));
        assertEquals(FecChoice.fixed(20), FecChoice.fromPreference("20"));
        assertEquals(FecChoice.fixed(0), FecChoice.fromPreference("0"));
        assertEquals(FecChoice.fixed(255), FecChoice.fromPreference("255"));
        for (String other : new String[] {null, "", "256", "-1", "20 %", "1000", "Auto"}) {
            assertEquals(other, FecChoice.HOST, FecChoice.fromPreference(other));
        }
    }

    // Foundation Sunshine takes V+'s x-ml-video.fecPercentage: -2 its own, -1 automatic, 0-100.
    @Test
    public void theFecChoiceIsVPlusFecPercentageForFoundation() {
        assertEquals(-2, FecChoice.HOST.summaryPercentage());
        assertEquals(-1, FecChoice.AUTO.summaryPercentage());
        assertEquals(0, FecChoice.fixed(0).summaryPercentage());
        assertEquals(20, FecChoice.fixed(20).summaryPercentage());
        assertEquals(100, FecChoice.fixed(255).summaryPercentage());
    }

    // 80 % of the measured link, never above the setting and never below 2 Mbit/s unless the
    // setting itself is lower.
    @Test
    public void aLinkTestOnlyLowersTheFirstBitrate() {
        assertEquals(20000, NvHTTP.probeStartKbps(40000, 25));
        assertEquals(40000, NvHTTP.probeStartKbps(40000, 500));
        assertEquals(40000, NvHTTP.probeStartKbps(40000, Double.POSITIVE_INFINITY));
        assertEquals(2000, NvHTTP.probeStartKbps(40000, 1));
        assertEquals(2000, NvHTTP.probeStartKbps(40000, 0));
        assertEquals(2000, NvHTTP.probeStartKbps(40000, Double.NaN));
        assertEquals(1500, NvHTTP.probeStartKbps(1500, 1));
    }

    // A link test counts only an octet-stream body of at most the requested size, and never an empty one.
    @Test
    public void aLinkTestBodyMustBeBinaryNonEmptyAndNoLargerThanAsked() throws Exception {
        MediaType binary = MediaType.get("application/octet-stream");
        assertEquals(1000, NvHTTP.readLinkProbe(ResponseBody.create(new byte[1000], binary)));
        assertEquals(NvHTTP.LINK_PROBE_BYTES,
                NvHTTP.readLinkProbe(ResponseBody.create(new byte[NvHTTP.LINK_PROBE_BYTES], binary)));
        assertThrows(java.io.IOException.class, () -> NvHTTP.readLinkProbe(ResponseBody.create(new byte[0], binary)));
        assertThrows(java.io.IOException.class, () -> NvHTTP.readLinkProbe(
                ResponseBody.create(new byte[NvHTTP.LINK_PROBE_BYTES + 1], binary)));
        assertThrows(java.io.IOException.class, () -> NvHTTP.readLinkProbe(
                ResponseBody.create(new byte[1000], MediaType.get("text/html"))));
    }

    @Test
    public void renderScaleDoesNotChangeTheStreamViewport() throws Exception {
        ConnectionContext context = context("butterpollo-serverinfo.xml");
        context.streamConfig = new StreamConfiguration.Builder().setLaunchRefreshRate(144)
                .setClientRefreshRateX100(24000).setVirtualDisplay(true, 150).build();
        HttpUrl query = query(context, false);
        assertEquals("1968x2184x144", query.queryParameter("mode"));
        assertEquals("150", query.queryParameter("scaleFactor"));
        assertNull(query.queryParameter("hdrMode"));
    }

    @Test
    public void displayScaleGoesWhenChosenWithOrWithoutTheVirtualDisplaySwitch() throws Exception {
        // The host makes a virtual display of its own by default (per_client), so "Host virtual
        // display" off does not mean a physical display; the host ignores the scale on one.
        for (boolean virtualDisplay : new boolean[] {false, true}) {
            for (int dpi : new int[] {0, 200}) {
                ConnectionContext context = context("rubylight-serverinfo.xml");
                context.hostCaps = TestHosts.rubylightWithDisplayScale();
                context.streamConfig = new StreamConfiguration.Builder().setLaunchRefreshRate(120)
                        .setVirtualDisplay(virtualDisplay, 100).setVirtualDisplayDpi(dpi).build();
                String query = NvHTTP.getLaunchQuery(context, 42, false);
                String expected = dpi != 0 ? "200" : null;
                assertEquals(virtualDisplay + "/" + dpi, expected, query(context, false).queryParameter("rlDisplayScale"));
                if (expected != null) {
                    // The same order as the shared Rust LaunchRequest; the render resolution stays 100 %.
                    assertTrue(query, query.contains(virtualDisplay ?
                            "&virtualDisplay=1&scaleFactor=100&rlDisplayScale=200&" : "&rlDisplayScale=200&"));
                    assertEquals(query, virtualDisplay, query.contains("virtualDisplay"));
                }
            }
        }
    }

    @Test
    public void hostsWithoutDisplayScaleVersionGetNoDisplayScale() throws Exception {
        // Older Rubylight hosts (2.6.x, 3.0.0-rc.5) ignore rlDisplayScale, so a chosen scale would silently do nothing.
        ConnectionContext context = context("rubylight-serverinfo.xml");
        assertEquals(HostCapabilities.Availability.unsupported(HostCapabilities.Reason.UPDATE_HOST),
                context.hostCaps.availability(HostCapabilities.Feature.DISPLAY_SCALE));
        context.streamConfig = new StreamConfiguration.Builder().setLaunchRefreshRate(120)
                .setVirtualDisplay(true, 100).setVirtualDisplayDpi(200).build();
        HttpUrl query = query(context, false);
        assertEquals("1", query.queryParameter("virtualDisplay"));
        assertNull(query.queryParameter("rlDisplayScale"));
        ConnectionContext apollo = context("apollo-serverinfo.xml");
        apollo.streamConfig = new StreamConfiguration.Builder().setLaunchRefreshRate(120)
                .setVirtualDisplay(true, 100).setVirtualDisplayDpi(200).build();
        assertNull(query(apollo, false).queryParameter("rlDisplayScale"));
    }

    @Test
    public void customFractionalRateReachesHostsThatReadIt() throws Exception {
        // Sunshine reads the rate with atoi and would turn 59.94 into 59.
        for (String host : new String[] {"butterpollo-serverinfo.xml", "rubylight-serverinfo.xml", "apollo-serverinfo.xml",
                "vibepollo-serverinfo.xml", "sunshine-serverinfo.xml", "gamestream-serverinfo.xml"}) {
            ConnectionContext context = context(host);
            context.streamConfig = new StreamConfiguration.Builder().setLaunchRefreshRate(60)
                    .setLaunchRefreshRateX100(5994).setClientRefreshRateX100(12000).build();
            assertEquals(host, host.startsWith("sunshine") || host.startsWith("gamestream") ?
                    "1968x2184x60" : "1968x2184x59.94", query(context, false).queryParameter("mode"));
        }
    }

    @Test
    public void clientCadenceMatchesFractionalLaunchOnlyWhenTheStreamRateMatches() throws Exception {
        for (String host : new String[] {"butterpollo-serverinfo.xml", "rubylight-serverinfo.xml",
                "apollo-serverinfo.xml", "vibepollo-serverinfo.xml", "sunshine-serverinfo.xml", "gamestream-serverinfo.xml"}) {
            ConnectionContext context = context(host);
            boolean fractional = context.hostCaps.has(HostCapabilities.Feature.FRACTIONAL_REFRESH);
            assertEquals(host, fractional ? 119880 : 120000, NvHTTP.getStreamFrameRateMilliHz(context));
            for (int streamFps : new int[] {59, 60}) {
                context.streamConfig = new StreamConfiguration.Builder().setRefreshRate(streamFps)
                        .setLaunchRefreshRate(60).setLaunchRefreshRateX100(5994)
                        .setClientRefreshRateX100(12000).build();
                assertEquals(host, fractional && streamFps == 60 ? 59940 : streamFps * 1000,
                        NvHTTP.getStreamFrameRateMilliHz(context));
            }
        }
    }

    @Test
    public void unrelatedOrInvalidDisplayCadenceCannotOverrideTheStream() throws Exception {
        ConnectionContext context = context("butterpollo-serverinfo.xml");
        for (int explicit : new int[] {0, Integer.MAX_VALUE, -1}) {
            context.streamConfig = new StreamConfiguration.Builder().setRefreshRate(60)
                    .setLaunchRefreshRate(60).setLaunchRefreshRateX100(explicit)
                    .setClientRefreshRateX100(11988).build();
            assertEquals(60000, NvHTTP.getStreamFrameRateMilliHz(context));
            assertEquals("1968x2184x60", query(context, false).queryParameter("mode"));
        }
    }

    @Test
    public void sunshineDoesNotReceiveApolloDisplayExtensions() throws Exception {
        HttpUrl query = query(context("sunshine-serverinfo.xml"), false);
        assertEquals("1968x2184x120", query.queryParameter("mode"));
        assertNull(query.queryParameter("virtualDisplay"));
        assertNull(query.queryParameter("scaleFactor"));
    }

    @Test
    public void apolloGetsThePanelRateAndItsVirtualDisplay() throws Exception {
        HttpUrl query = query(context("apollo-serverinfo.xml"), false);
        assertEquals("1968x2184x119.88", query.queryParameter("mode"));
        assertEquals("1", query.queryParameter("virtualDisplay"));
        assertEquals("100", query.queryParameter("scaleFactor"));
    }

    @Test
    public void unavailableDriverAndDisabledPreferencePreserveHostPolicy() throws Exception {
        ConnectionContext context = context("butterpollo-serverinfo.xml");
        context.hostCaps = HostCapabilities.fromServerInfo(fixture("butterpollo-serverinfo.xml")
                .replace("<VirtualDisplayDriverReady>true", "<VirtualDisplayDriverReady>false"));
        assertNull(query(context, false).queryParameter("virtualDisplay"));
        context.hostCaps = TestHosts.fixture("butterpollo-serverinfo.xml");
        context.streamConfig = new StreamConfiguration.Builder().build();
        assertNull(query(context, false).queryParameter("virtualDisplay"));
        assertNull(query(context, false).queryParameter("scaleFactor"));
    }

    @Test
    public void displayChoiceIsSentWithItsVirtualDisplayRequest() throws Exception {
        for (HostDisplayChoice choice : new HostDisplayChoice[] {HostDisplayChoice.EXCLUSIVE,
                HostDisplayChoice.EXTENDED_PRIMARY, HostDisplayChoice.EXTENDED,
                HostDisplayChoice.EXTENDED_ISOLATED, HostDisplayChoice.EXTENDED_PRIMARY_ISOLATED}) {
            ConnectionContext context = context("rubylight-serverinfo.xml");
            // The choice asks for a virtual display even with the setting off.
            context.streamConfig = new StreamConfiguration.Builder().setLaunchRefreshRate(120)
                    .setVirtualDisplay(false, 125).build();
            context.hostDisplay = choice;
            HttpUrl query = query(context, false);
            assertEquals(choice.wireValue, query.queryParameter("hostDisplay"));
            assertEquals("1", query.queryParameter("virtualDisplay"));
            assertEquals("125", query.queryParameter("scaleFactor"));
        }
    }

    @Test
    public void physicalChoiceOverridesTheVirtualDisplaySetting() throws Exception {
        ConnectionContext context = context("rubylight-serverinfo.xml");
        context.hostDisplay = HostDisplayChoice.PHYSICAL;
        HttpUrl query = query(context, false);
        assertEquals("physical", query.queryParameter("hostDisplay"));
        assertNull(query.queryParameter("virtualDisplay"));
        assertNull(query.queryParameter("scaleFactor"));
    }

    @Test
    public void hostDefaultSendsNoChoiceAndKeepsTheVirtualDisplaySetting() throws Exception {
        ConnectionContext context = context("rubylight-serverinfo.xml");
        assertEquals(HostDisplayChoice.HOST_DEFAULT, context.hostDisplay);
        HttpUrl query = query(context, false);
        assertNull(query.queryParameter("hostDisplay"));
        assertEquals("1", query.queryParameter("virtualDisplay"));
        context.hostDisplay = null;
        assertNull(query(context, false).queryParameter("hostDisplay"));
    }

    @Test
    public void hostsWithoutDisplayModesIgnoreASavedChoiceAndFollowTheSwitch() throws Exception {
        // A choice saved before the picker was limited to such hosts must not become virtualDisplay=1.
        for (String host : new String[] {"apollo-serverinfo.xml", "vibepollo-serverinfo.xml", "butterpollo-serverinfo.xml",
                "sunshine-serverinfo.xml", "gamestream-serverinfo.xml"}) {
            for (boolean virtualDisplay : new boolean[] {false, true}) {
                ConnectionContext context = context(host);
                context.streamConfig = new StreamConfiguration.Builder().setLaunchRefreshRate(120)
                        .setVirtualDisplay(virtualDisplay, 100).build();
                context.hostDisplay = HostDisplayChoice.EXCLUSIVE;
                HttpUrl query = query(context, false);
                assertNull(host, query.queryParameter("hostDisplay"));
                boolean hostHasOne = context.hostCaps.has(HostCapabilities.Feature.VIRTUAL_DISPLAY);
                assertEquals(host, virtualDisplay && hostHasOne ? "1" : null, query.queryParameter("virtualDisplay"));
                assertEquals(host, virtualDisplay, NvHTTP.wantsVirtualDisplay(context, NvApp.Role.STREAM));
            }
        }
    }

    @Test
    public void vrrIsRequestedOnlyFromHostsThatReadIt() throws Exception {
        // Vibepollo reads clientVrrRequested and switches its virtual display for it; Rubylight
        // also reads the older vrr; Apollo, Sunshine and GameStream read neither.
        String[][] hosts = {{"butterpollo-serverinfo.xml", "1", "1"}, {"rubylight-serverinfo.xml", "1", "1"},
                {"vibepollo-serverinfo.xml", "1", null}, {"apollo-serverinfo.xml", null, null},
                {"sunshine-serverinfo.xml", null, null}, {"gamestream-serverinfo.xml", null, null}};
        for (String[] host : hosts) {
            for (String verb : new String[] {"launch", "resume"}) {
                ConnectionContext context = context(host[0]);
                HttpUrl query = HttpUrl.get("https://host/" + verb + "?" + NvHTTP.getLaunchQuery(context, 42, false));
                assertNull(query.queryParameter("clientVrrRequested"));
                assertNull(query.queryParameter("vrr"));
                context.streamConfig = new StreamConfiguration.Builder().setLaunchRefreshRate(120)
                        .setClientRefreshRateX100(12000).setVrr(true).build();
                query = HttpUrl.get("https://host/" + verb + "?" + NvHTTP.getLaunchQuery(context, 42, false));
                assertEquals(host[0], host[1], query.queryParameter("clientVrrRequested"));
                assertEquals(host[0], host[2], query.queryParameter("vrr"));
            }
        }
    }

    @Test
    public void legacyNvidiaRetainsSopsWorkarounds() throws Exception {
        ConnectionContext context = context("gamestream-serverinfo.xml");
        context.isNvidiaServerSoftware = true;
        HttpUrl query = query(context, false);
        assertEquals("1968x2184x0", query.queryParameter("mode"));
        assertEquals("0", query.queryParameter("sops"));
    }

    @Test
    public void stockLaunchRetainsSurroundAudioControllerAndEncryptionParameters() throws Exception {
        ConnectionContext context = context("sunshine-serverinfo.xml");
        context.streamConfig = new StreamConfiguration.Builder().setLaunchRefreshRate(60)
                .setAudioConfiguration(new com.limelight.nvstream.jni.MoonBridge.AudioConfiguration(6, 0x3F))
                .enableLocalAudioPlayback(true).setAttachedGamepadMask(5)
                .setPersistGamepadsAfterDisconnect(true).build();
        HttpUrl query = query(context, false);
        assertEquals("1", query.queryParameter("localAudioPlayMode"));
        assertEquals(Integer.toString((0x3F << 16) | 6), query.queryParameter("surroundAudioInfo"));
        assertEquals("5", query.queryParameter("remoteControllersBitmap"));
        assertEquals("5", query.queryParameter("gcmap"));
        assertEquals("1", query.queryParameter("gcpersist"));
        assertEquals("00000000000000000000000000000000", query.queryParameter("rikey"));
        assertEquals("-123", query.queryParameter("rikeyid"));
    }

    @Test
    public void appListAcceptsInheritedExtensionFields() throws Exception {
        LinkedList<NvApp> apps = NvHTTP.getAppListByReader(new StringReader(fixture("butterpollo-applist.xml")));
        assertEquals(2, apps.size());
        assertEquals("Desktop", apps.get(0).getAppName());
        assertEquals(42, apps.get(0).getAppId());
        assertTrue(apps.get(0).isHdrSupported());
        assertEquals("Game & tools", apps.get(1).getAppName());
        assertEquals("00000000-0000-0000-0000-000000000042", apps.get(0).getAppUuid());
        assertEquals(0, apps.get(0).getHostIndex());
        assertEquals("2", apps.get(1).getArtVersion());
    }

    @Test
    public void appUuidLaunchIsEscapedAndRunningIdentityWinsOverReusedNumericIds() throws Exception {
        ConnectionContext context = context("butterpollo-serverinfo.xml");
        NvApp app = context.streamConfig.getApp();
        assertNull(query(context, false).queryParameter("appuuid"));
        app.setAppUuid("app &?=# identity");
        assertEquals("app &?=# identity", query(context, false).queryParameter("appuuid"));
        assertEquals(1, query(context, false).queryParameterValues("appid").size());
        app.setAppId(42);
        assertTrue(app.matchesRunningApp(43, app.getAppUuid()));
        assertFalse(app.matchesRunningApp(42, "different-app"));
        assertTrue(app.matchesRunningApp(42, null));
        assertFalse(app.matchesRunningApp(0, null));
        // Apollo's input-only mode reports only the running game's UUID.
        assertTrue(app.matchesRunningApp(0, app.getAppUuid()));
        assertFalse(app.matchesRunningApp(0, "different-app"));
    }

    @Test
    public void advertisedHostOrderWinsWhileStockAndInvalidIndicesSortByName() throws Exception {
        LinkedList<NvApp> apps = NvHTTP.getAppListByReader(new StringReader(
                "<root status_code=\"200\">" +
                        "<App><ID>1</ID><AppTitle>Zulu</AppTitle><IDX>0</IDX></App>" +
                        "<App><ID>2</ID><AppTitle>Alpha</AppTitle><IDX>1</IDX></App>" +
                        "<App><ID>3</ID><AppTitle>Beta</AppTitle><IDX>-1</IDX></App>" +
                        "<App><ID>4</ID><AppTitle>Charlie</AppTitle><IDX>oops</IDX></App>" +
                        "<App><ID>5</ID><AppTitle>Delta</AppTitle><IDX>2147483648</IDX></App>" +
                        "</root>"));
        java.util.Collections.reverse(apps);
        java.util.Collections.sort(apps, NvApp::compareForDisplay);
        assertEquals("Zulu", apps.get(0).getAppName());
        assertEquals("Alpha", apps.get(1).getAppName());
        assertEquals("Beta", apps.get(2).getAppName());
        assertEquals(Integer.MAX_VALUE, apps.get(4).getHostIndex());
        assertTrue(new NvApp("alpha").compareForDisplay(new NvApp("Beta")) < 0);
    }

    @Test
    public void artworkCacheTracksOpaqueVersionAndIdentityWithoutUsingHostPaths() {
        NvApp app = new NvApp("Game", 42, false);
        assertEquals("42", app.getAssetCacheKey());
        app.setAppUuid("a");
        String original = app.getAssetCacheKey();
        app.setArtVersion("../../new/art?file=1");
        String updated = app.getAssetCacheKey();
        assertNotEquals(original, updated);
        assertTrue(updated.matches("42-[0-9a-f-]{36}"));
        app.setAppUuid("b");
        assertNotEquals(updated, app.getAssetCacheKey());
    }

    @Test
    public void launchQueryCarriesOnlyWhatEachHostReads() throws Exception {
        // host, mode, virtualDisplay, scaleFactor, hostDisplay, clientVrrRequested, vrr
        String[][] hosts = {
                {"sunshine-serverinfo.xml", "1968x2184x60", null, null, null, null, null},
                {"apollo-serverinfo.xml", "1968x2184x59.94", "1", "125", null, null, null},
                {"apollo-http-serverinfo.xml", "1968x2184x59.94", null, null, null, null, null},
                {"vibepollo-serverinfo.xml", "1968x2184x59.94", "1", "125", null, "1", null},
                {"gamestream-serverinfo.xml", "1968x2184x60", null, null, null, null, null},
                {"butterpollo-serverinfo.xml", "1968x2184x59.94", "1", "125", null, "1", "1"},
                {"rubylight-serverinfo.xml", "1968x2184x59.94", "1", "125", "exclusive", "1", "1"},
        };
        for (String[] host : hosts) {
            ConnectionContext context = context(host[0]);
            context.isNvidiaServerSoftware = context.hostCaps.family() == HostCapabilities.Family.GAMESTREAM;
            context.streamConfig = new StreamConfiguration.Builder().setLaunchRefreshRate(60)
                    .setLaunchRefreshRateX100(5994).setClientRefreshRateX100(12000)
                    .setVirtualDisplay(true, 125).setVrr(true).build();
            context.hostDisplay = HostDisplayChoice.EXCLUSIVE;
            HttpUrl query = query(context, false);
            assertEquals(host[0], host[1], query.queryParameter("mode"));
            assertEquals(host[0], host[2], query.queryParameter("virtualDisplay"));
            assertEquals(host[0], host[3], query.queryParameter("scaleFactor"));
            assertEquals(host[0], host[4], query.queryParameter("hostDisplay"));
            assertEquals(host[0], host[5], query.queryParameter("clientVrrRequested"));
            assertEquals(host[0], host[6], query.queryParameter("vrr"));
            assertEquals(host[0], "42", query.queryParameter("appid"));
        }
    }

    @Test
    public void computerDetailsCarryTheHostsCapabilities() throws Exception {
        assertTrue(http().getComputerDetails(fixture("gamestream-serverinfo.xml")).nvidiaServer);
        for (String host : new String[] {"sunshine", "apollo", "vibepollo", "rubylight"}) {
            ComputerDetails details = http().getComputerDetails(fixture(host + "-serverinfo.xml"));
            assertFalse(host, details.nvidiaServer);
            assertEquals(host, TestHosts.fixture(host + "-serverinfo.xml"), details.caps);
            assertEquals(host, details.caps, new ComputerDetails(details).caps);
        }
        // Details that never reached the host (mDNS, a manual address) keep what is known.
        ComputerDetails known = http().getComputerDetails(fixture("rubylight-serverinfo.xml"));
        known.update(new ComputerDetails());
        assertEquals(HostCapabilities.Family.RUBYLIGHT, known.caps.family());
        known.update(http().getComputerDetails(fixture("sunshine-serverinfo.xml")));
        assertEquals(HostCapabilities.Family.SUNSHINE, known.caps.family());
    }

    @Test
    public void controlTilesAreOrdinaryAppsOnHostsWithoutThem() throws Exception {
        String list = "<root status_code=\"200\"><App><ID>2147483505</ID><AppTitle>Remote Monitor</AppTitle></App>" +
                "<App><ID>7</ID><AppTitle>Game</AppTitle><UUID>9a1c5a25-58fe-40e0-b9aa-7d3f00000004</UUID></App></root>";
        for (String host : new String[] {"sunshine", "apollo", "gamestream"}) {
            for (NvApp app : NvHTTP.getAppListByReader(new StringReader(list), TestHosts.fixture(host + "-serverinfo.xml"))) {
                assertEquals(host, NvApp.Control.NONE, app.getControl());
                assertEquals(host, NvApp.Role.STREAM, app.getRole());
            }
        }
        for (HostCapabilities caps : new HostCapabilities[] {TestHosts.rubylight(), TestHosts.fixture("vibepollo-serverinfo.xml"),
                HostCapabilities.UNKNOWN}) {
            LinkedList<NvApp> apps = NvHTTP.getAppListByReader(new StringReader(list), caps);
            assertEquals(NvApp.Control.MONITOR, apps.get(0).getControl());
            assertEquals(NvApp.Control.TERMINATE, apps.get(1).getControl());
        }
    }

    @Test
    public void emptyAppListAllowsRootWhitespaceAndExtensionText() throws Exception {
        assertTrue(NvHTTP.getAppListByReader(new StringReader(
                "<root status_code=\"200\">\n<Extension>host data</Extension>\n</root>\n")).isEmpty());
    }
}

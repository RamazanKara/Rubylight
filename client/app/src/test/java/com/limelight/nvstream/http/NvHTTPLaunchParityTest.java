/** Launch-oriented parity tests for NvHTTP over loopback TestHttpServer. */
package com.limelight.nvstream.http;

import android.app.Application;

import com.limelight.nvstream.ConnectionContext;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.preferences.HostDisplayChoice;
import com.limelight.utils.ClipboardSync;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.io.IOException;
import java.io.InputStream;
import java.io.StringReader;
import java.net.URLDecoder;
import java.nio.charset.StandardCharsets;
import java.security.PrivateKey;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.LinkedList;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Set;

import javax.crypto.spec.SecretKeySpec;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class NvHTTPLaunchParityTest {
    private static final class Query {
        private final Map<String, List<String>> params = new LinkedHashMap<>();

        Query(String rawQuery) {
            for (String pair : rawQuery.split("&")) {
                if (pair.isEmpty()) continue;
                int eq = pair.indexOf('=');
                String key = URLDecoder.decode(eq < 0 ? pair : pair.substring(0, eq), StandardCharsets.UTF_8);
                String val = eq < 0 ? "" : URLDecoder.decode(pair.substring(eq + 1), StandardCharsets.UTF_8);
                params.computeIfAbsent(key, k -> new ArrayList<>()).add(val);
            }
        }

        String queryParameter(String name) {
            List<String> vals = params.get(name);
            return vals == null || vals.isEmpty() ? null : vals.get(0);
        }

        List<String> queryParameterValues(String name) {
            List<String> vals = params.get(name);
            return vals == null ? List.of() : vals;
        }
    }

    private NvHTTP http() throws IOException {
        return new NvHTTP(new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984,
                "1234567890abcdef", null, new LimelightCryptoProvider() {
            public X509Certificate getClientCertificate() { return null; }
            public PrivateKey getClientPrivateKey() { return null; }
            public byte[] getPemEncodedClientCertificate() { return new byte[0]; }
            public String encodeBase64String(byte[] data) { return ""; }
        });
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

    private Query query(ConnectionContext context, boolean hdr) {
        return new Query(NvHTTP.getLaunchQuery(context, 42, hdr));
    }

    @Test
    public void pyrowaveServerCapabilitiesRemainSeparateFromLaunchHdr() throws Exception {
        ConnectionContext context = context("butterpollo-serverinfo.xml");
        assertEquals(0x07830301L, http().getServerCodecModeSupport(fixture("butterpollo-serverinfo.xml")));
        assertNull(query(context, false).queryParameter("hdrMode"));
        assertEquals("1", query(context, true).queryParameter("hdrMode"));
        assertFalse(NvHTTP.getLaunchQuery(context, 42, true).toLowerCase(Locale.ROOT).contains("pyrowave"));
    }

    @Test
    public void rubylightLaunchPreservesNativePixelsAndFractionalHighRefresh() throws Exception {
        ConnectionContext context = context("butterpollo-serverinfo.xml");
        Locale previous = Locale.getDefault();
        try {
            Locale.setDefault(Locale.GERMANY);
            Query query = query(context, true);
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
        assertNull(query(context, false).queryParameter("hdrMode"));
        assertEquals("1", query(context, true).queryParameter("hdrMode"));
        context.streamConfig = new StreamConfiguration.Builder().setLaunchRefreshRate(60).setSdr10Bit(true).build();
        context.negotiated10Bit = true;
        assertEquals("0", query(context, false).queryParameter("hdrMode"));
        assertEquals("1", query(context, true).queryParameter("hdrMode"));
        context.negotiated10Bit = false;
        assertNull(query(context, false).queryParameter("hdrMode"));
    }

    @Test
    public void hdr10OnlyAsksAHostThatCanToLeaveOutDynamicMetadata() throws Exception {
        ConnectionContext context = context("rubylight-30-serverinfo.xml");
        context.hostCaps = TestHosts.caps(TestHosts.serverInfo("rubylight-30-serverinfo.xml")
                .replace("</root>", "<HdrDynamicControl>1</HdrDynamicControl></root>"));
        assertNull(query(context, true).queryParameter("rlHdrDynamic"));
        context.streamConfig = new StreamConfiguration.Builder().setLaunchRefreshRate(60).setHdrStaticOnly(true).build();
        assertEquals("0", query(context, true).queryParameter("rlHdrDynamic"));
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
                assertEquals(server, plain, asked);
                continue;
            }
            assertEquals(plain.replace("&corever=1", "") + "&rlFecPercentage=auto&corever=1", asked);
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
            assertEquals(server, plain.replace("&corever=1", "") + "&rlRateChange=1&corever=1",
                    NvHTTP.getLaunchQuery(context, 42, false));
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
            assertNull(server, query(context, true).queryParameter("maxBrightness"));
            context.streamConfig = new StreamConfiguration.Builder()
                    .setResolution(1968, 2184).setRefreshRate(120).setLaunchRefreshRate(120)
                    .setClientRefreshRateX100(11988).setVirtualDisplay(true, 100)
                    .setDisplayLuminance(luminance).build();
            Query query = query(context, true);
            if (context.hostCaps.family() == HostCapabilities.Family.RUBYLIGHT) {
                assertNull(server, query.queryParameter("maxBrightness"));
                continue;
            }
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
            assertEquals(server, plain.replace("&corever=1", "") + "&" + ClipboardSync.LAUNCH_PARAMETER + "=1&corever=1", asked);

            // The Remote Input and Remote Monitor tiles: asking and syncing go together.
            for (int appId : new int[] {2147483506, 2147483505}) {
                NvApp app = new NvApp("Tile", appId, false);
                boolean asks = app.getRole() == NvApp.Role.INPUT_ONLY;
                assertTrue(asks || app.getRole() == NvApp.Role.REMOTE_MONITOR);
                context.streamConfig = new StreamConfiguration.Builder().setApp(app).setClipboardSync(true).build();
                String message = server + " " + app.getRole();
                assertEquals(message, asks, NvHTTP.getLaunchQuery(context, appId, false)
                        .contains(ClipboardSync.LAUNCH_PARAMETER + "=1"));
                assertEquals(message, asks, NvHTTP.asksClipboardSync(context));
            }
        }
        assertTrue(seen.contains(HostCapabilities.Family.RUBYLIGHT));
        assertTrue(seen.size() > 1);
    }

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

    @Test
    public void theFecChoiceIsVPlusFecPercentageForFoundation() {
        assertEquals(-2, FecChoice.HOST.summaryPercentage());
        assertEquals(-1, FecChoice.AUTO.summaryPercentage());
        assertEquals(0, FecChoice.fixed(0).summaryPercentage());
        assertEquals(20, FecChoice.fixed(20).summaryPercentage());
        assertEquals(100, FecChoice.fixed(255).summaryPercentage());
    }

    @Test
    public void renderScaleDoesNotChangeTheStreamViewport() throws Exception {
        ConnectionContext context = context("butterpollo-serverinfo.xml");
        context.streamConfig = new StreamConfiguration.Builder().setLaunchRefreshRate(144)
                .setClientRefreshRateX100(24000).setVirtualDisplay(true, 150).build();
        Query query = query(context, false);
        assertEquals("1968x2184x144", query.queryParameter("mode"));
        assertEquals("150", query.queryParameter("scaleFactor"));
        assertNull(query.queryParameter("hdrMode"));
    }

    @Test
    public void displayScaleGoesWhenChosenWithOrWithoutTheVirtualDisplaySwitch() throws Exception {
        for (boolean virtualDisplay : new boolean[] {false, true}) {
            for (int dpi : new int[] {0, 200}) {
                ConnectionContext context = context("rubylight-serverinfo.xml");
                context.hostCaps = TestHosts.rubylightWithDisplayScale();
                context.streamConfig = new StreamConfiguration.Builder().setLaunchRefreshRate(120)
                        .setVirtualDisplay(virtualDisplay, 100).setVirtualDisplayDpi(dpi).build();
                String queryStr = NvHTTP.getLaunchQuery(context, 42, false);
                String expected = dpi != 0 ? "200" : null;
                assertEquals(virtualDisplay + "/" + dpi, expected, query(context, false).queryParameter("rlDisplayScale"));
                if (expected != null) {
                    assertTrue(queryStr, queryStr.contains(virtualDisplay ?
                            "&virtualDisplay=1&scaleFactor=100&rlDisplayScale=200&" : "&rlDisplayScale=200&"));
                    assertEquals(queryStr, virtualDisplay, queryStr.contains("virtualDisplay"));
                }
            }
        }
    }

    @Test
    public void hostsWithoutDisplayScaleVersionGetNoDisplayScale() throws Exception {
        ConnectionContext context = context("rubylight-serverinfo.xml");
        assertEquals(HostCapabilities.Availability.unsupported(HostCapabilities.Reason.UPDATE_HOST),
                context.hostCaps.availability(HostCapabilities.Feature.DISPLAY_SCALE));
        context.streamConfig = new StreamConfiguration.Builder().setLaunchRefreshRate(120)
                .setVirtualDisplay(true, 100).setVirtualDisplayDpi(200).build();
        Query query = query(context, false);
        assertEquals("1", query.queryParameter("virtualDisplay"));
        assertNull(query.queryParameter("rlDisplayScale"));
        ConnectionContext apollo = context("apollo-serverinfo.xml");
        apollo.streamConfig = new StreamConfiguration.Builder().setLaunchRefreshRate(120)
                .setVirtualDisplay(true, 100).setVirtualDisplayDpi(200).build();
        assertNull(query(apollo, false).queryParameter("rlDisplayScale"));
    }

    @Test
    public void customFractionalRateReachesHostsThatReadIt() throws Exception {
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
        Query query = query(context("sunshine-serverinfo.xml"), false);
        assertEquals("1968x2184x120", query.queryParameter("mode"));
        assertNull(query.queryParameter("virtualDisplay"));
        assertNull(query.queryParameter("scaleFactor"));
    }

    @Test
    public void apolloGetsThePanelRateAndItsVirtualDisplay() throws Exception {
        Query query = query(context("apollo-serverinfo.xml"), false);
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
            context.streamConfig = new StreamConfiguration.Builder().setLaunchRefreshRate(120)
                    .setVirtualDisplay(false, 125).build();
            context.hostDisplay = choice;
            Query query = query(context, false);
            assertEquals(choice.wireValue, query.queryParameter("hostDisplay"));
            assertEquals("1", query.queryParameter("virtualDisplay"));
            assertEquals("125", query.queryParameter("scaleFactor"));
        }
    }

    @Test
    public void physicalChoiceOverridesTheVirtualDisplaySetting() throws Exception {
        ConnectionContext context = context("rubylight-serverinfo.xml");
        context.hostDisplay = HostDisplayChoice.PHYSICAL;
        Query query = query(context, false);
        assertEquals("physical", query.queryParameter("hostDisplay"));
        assertNull(query.queryParameter("virtualDisplay"));
        assertNull(query.queryParameter("scaleFactor"));
    }

    @Test
    public void hostDefaultSendsNoChoiceAndKeepsTheVirtualDisplaySetting() throws Exception {
        ConnectionContext context = context("rubylight-serverinfo.xml");
        assertEquals(HostDisplayChoice.HOST_DEFAULT, context.hostDisplay);
        Query query = query(context, false);
        assertNull(query.queryParameter("hostDisplay"));
        assertEquals("1", query.queryParameter("virtualDisplay"));
        context.hostDisplay = null;
        assertNull(query(context, false).queryParameter("hostDisplay"));
    }

    @Test
    public void hostsWithoutDisplayModesIgnoreASavedChoiceAndFollowTheSwitch() throws Exception {
        for (String host : new String[] {"apollo-serverinfo.xml", "vibepollo-serverinfo.xml", "butterpollo-serverinfo.xml",
                "sunshine-serverinfo.xml", "gamestream-serverinfo.xml"}) {
            for (boolean virtualDisplay : new boolean[] {false, true}) {
                ConnectionContext context = context(host);
                context.streamConfig = new StreamConfiguration.Builder().setLaunchRefreshRate(120)
                        .setVirtualDisplay(virtualDisplay, 100).build();
                context.hostDisplay = HostDisplayChoice.EXCLUSIVE;
                Query query = query(context, false);
                assertNull(host, query.queryParameter("hostDisplay"));
                boolean hostHasOne = context.hostCaps.has(HostCapabilities.Feature.VIRTUAL_DISPLAY);
                assertEquals(host, virtualDisplay && hostHasOne ? "1" : null, query.queryParameter("virtualDisplay"));
                assertEquals(host, virtualDisplay, NvHTTP.wantsVirtualDisplay(context, NvApp.Role.STREAM));
            }
        }
    }

    @Test
    public void vrrIsRequestedOnlyFromHostsThatReadIt() throws Exception {
        String[][] hosts = {{"butterpollo-serverinfo.xml", "1", "1"}, {"rubylight-serverinfo.xml", "1", "1"},
                {"vibepollo-serverinfo.xml", "1", null}, {"apollo-serverinfo.xml", null, null},
                {"sunshine-serverinfo.xml", null, null}, {"gamestream-serverinfo.xml", null, null}};
        for (String[] host : hosts) {
            for (String verb : new String[] {"launch", "resume"}) {
                ConnectionContext context = context(host[0]);
                Query query = new Query(NvHTTP.getLaunchQuery(context, 42, false));
                assertNull(query.queryParameter("clientVrrRequested"));
                assertNull(query.queryParameter("vrr"));
                context.streamConfig = new StreamConfiguration.Builder().setLaunchRefreshRate(120)
                        .setClientRefreshRateX100(12000).setVrr(true).build();
                query = new Query(NvHTTP.getLaunchQuery(context, 42, false));
                assertEquals(host[0], host[1], query.queryParameter("clientVrrRequested"));
                assertEquals(host[0], host[2], query.queryParameter("vrr"));
            }
        }
    }

    @Test
    public void legacyNvidiaRetainsSopsWorkarounds() throws Exception {
        ConnectionContext context = context("gamestream-serverinfo.xml");
        context.isNvidiaServerSoftware = true;
        Query query = query(context, false);
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
        Query query = query(context, false);
        assertEquals("1", query.queryParameter("localAudioPlayMode"));
        assertEquals(Integer.toString((0x3F << 16) | 6), query.queryParameter("surroundAudioInfo"));
        assertEquals("5", query.queryParameter("remoteControllersBitmap"));
        assertEquals("5", query.queryParameter("gcmap"));
        assertEquals("1", query.queryParameter("gcpersist"));
        assertEquals("00000000000000000000000000000000", query.queryParameter("rikey"));
        assertEquals("-123", query.queryParameter("rikeyid"));
    }

    @Test
    public void sevenOneFourSurroundAudioInfoIsSentUnsigned() throws Exception {
        ConnectionContext context = context("rubylight-30-serverinfo.xml");
        context.streamConfig = new StreamConfiguration.Builder().setLaunchRefreshRate(60)
                .setAudioConfiguration(new com.limelight.nvstream.jni.MoonBridge.AudioConfiguration(12, 0xF63F))
                .build();
        Query query = query(context, false);
        assertEquals("4131323916", query.queryParameter("surroundAudioInfo"));
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
            Query query = query(context, false);
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

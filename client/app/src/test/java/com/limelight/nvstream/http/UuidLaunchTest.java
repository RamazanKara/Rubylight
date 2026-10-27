package com.limelight.nvstream.http;

import android.app.Application;

import com.limelight.nvstream.ConnectionContext;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.NvConnectionListener;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.av.video.VideoDecoderRenderer;
import com.limelight.nvstream.jni.MoonBridge;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.util.ReflectionHelpers;

import java.lang.reflect.Proxy;
import java.util.ArrayList;
import java.util.List;

import okhttp3.HttpUrl;

import static org.junit.Assert.*;

/**
 * The launch query a UUID tile sends once NvConnection has resolved its numeric ID. It sits in
 * this package to build the query with {@link NvHTTP#getLaunchQuery}.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, shadows = UuidLaunchTest.FakeHost.class)
public class UuidLaunchTest {
    private static final String UUID = "0F3C2B1A-1111-4222-8333-944455556666";
    // Every permission but List, so the client cannot look the UUID up in the app list.
    private static final String NO_LIST = Long.toString(0xFFFFFFFFL & ~ComputerDetails.PERMISSION_LIST);
    private static final LimelightCryptoProvider CRYPTO = new LimelightCryptoProvider() {
        @Override public java.security.cert.X509Certificate getClientCertificate() { return null; }
        @Override public java.security.PrivateKey getClientPrivateKey() { return null; }
        @Override public byte[] getPemEncodedClientCertificate() { return new byte[0]; }
        @Override public String encodeBase64String(byte[] data) { return ""; }
    };

    @Before public void resetHost() {
        FakeHost.listed = null;
        FakeHost.requests.clear();
        FakeHost.launches.clear();
    }

    private static String serverInfo(String host, boolean list, boolean running) {
        String xml = TestHosts.serverInfo(host + "-serverinfo.xml");
        String permission = "<Permission>4294967295</Permission>";
        String idle = "<currentgame>0</currentgame><currentgameuuid></currentgameuuid><state>SUNSHINE_SERVER_FREE</state>";
        assertTrue(host, xml.contains(permission) && xml.contains(idle));
        if (!list) {
            xml = xml.replace(permission, "<Permission>" + NO_LIST + "</Permission>");
        }
        if (running) {
            xml = xml.replace(idle, "<currentgame>5</currentgame>" +
                    "<currentgameuuid>5A5A5A5A-0000-4000-8000-000000000005</currentgameuuid><state>SUNSHINE_SERVER_BUSY</state>");
        }
        return xml;
    }

    private static HttpUrl launch(String serverInfo) throws Exception {
        FakeHost.serverInfo = serverInfo;
        NvApp app = new NvApp("Game", 73, false);
        app.setAppUuid(UUID);
        NvConnection connection = new NvConnection(RuntimeEnvironment.getApplication(),
                new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "device",
                new StreamConfiguration.Builder().setApp(app).setResolution(1280, 720).setRefreshRate(60)
                        .setSupportedVideoFormats(MoonBridge.VIDEO_FORMAT_H264).build(), CRYPTO, null);
        NvConnectionListener listener = (NvConnectionListener) Proxy.newProxyInstance(
                NvConnectionListener.class.getClassLoader(), new Class<?>[] {NvConnectionListener.class},
                (proxy, method, args) -> null);
        connection.start(null, new VideoDecoderRenderer() {
            @Override public int setup(int format, int width, int height, int rate) { return -1; }
            @Override public void start() {}
            @Override public void stop() {}
            @Override public void cleanup() {}
            @Override public int getCapabilities() { return 0; }
            @Override public void setHdrMode(boolean enabled, byte[] metadata) {}
            @Override public int submitDecodeUnit(byte[] data, int length, int type, int frame, int frameType,
                    char latency, long received, long queued, long receivedNs, long presentationUs) { return 0; }
        }, listener);
        Thread worker = ReflectionHelpers.getField(connection, "connectionThread");
        worker.join(5000);
        if (worker.isAlive()) {
            connection.stop();
            worker.join(5000);
            fail("Connection startup did not finish");
        }
        assertEquals(1, FakeHost.launches.size());
        return FakeHost.launches.get(0);
    }

    @Test
    public void apolloKeepsTheTilesIdWhenItCannotListApps() throws Exception {
        // Apollo checks appid against its running app before it reads appuuid; zero on an idle
        // Apollo is taken as a resume and streams without starting the app.
        HttpUrl query = launch(serverInfo("apollo", false, false));
        assertEquals("73", query.queryParameter("appid"));
        assertEquals(UUID, query.queryParameter("appuuid"));
        assertEquals(List.of("launch"), FakeHost.requests);
    }

    @Test
    public void apolloReplacementLaunchCarriesTheIdAfterCancelling() throws Exception {
        // Apollo has no replacement confirmation: the game is cancelled, then the tile launched
        // on an idle host, where zero would again mean "resume".
        HttpUrl query = launch(serverInfo("apollo", false, true));
        assertEquals(List.of("cancel", "launch"), FakeHost.requests);
        assertEquals("73", query.queryParameter("appid"));
        assertEquals(UUID, query.queryParameter("appuuid"));
    }

    @Test
    public void apolloInputOnlyModeCountsTheGameItHidesAsRunning() throws Exception {
        String xml = TestHosts.serverInfo("apollo-input-only-serverinfo.xml");
        ComputerDetails details = new NvHTTP(new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984,
                "device", null, CRYPTO).getComputerDetails(xml);
        assertEquals(0, details.runningGameId);
        assertTrue(details.sessionIdle);
        assertTrue(details.hasRunningGame());
        NvApp game = new NvApp("Game", 73, false);
        game.setAppUuid(UUID);
        assertTrue(game.matchesRunningApp(details.runningGameId, details.runningGameUuid));
        NvApp other = new NvApp("Other", 74, false);
        other.setAppUuid("5A5A5A5A-0000-4000-8000-000000000005");
        assertFalse(other.matchesRunningApp(details.runningGameId, details.runningGameUuid));
    }

    @Test
    public void apolloInputOnlyLaunchKeepsTheTilesId() throws Exception {
        // The running game matches by UUID alone; its ID from serverinfo is 0 and must not replace the tile's.
        String xml = TestHosts.serverInfo("apollo-input-only-serverinfo.xml")
                .replace("<Permission>4294967295</Permission>", "<Permission>" + NO_LIST + "</Permission>");
        HttpUrl query = launch(xml);
        assertEquals(List.of("launch"), FakeHost.requests);
        assertEquals("73", query.queryParameter("appid"));
        assertEquals(UUID, query.queryParameter("appuuid"));
    }

    @Test
    public void hostsThatLookTheUuidUpFirstGetNoStaleId() throws Exception {
        for (String host : new String[] {"vibepollo", "rubylight"}) {
            for (boolean running : new boolean[] {false, true}) {
                resetHost();
                HttpUrl query = launch(serverInfo(host, false, running));
                // The host confirms replacement itself, so nothing is cancelled first.
                assertEquals(host, List.of("launch"), FakeHost.requests);
                assertEquals(host, "0", query.queryParameter("appid"));
                assertEquals(host, UUID, query.queryParameter("appuuid"));
            }
        }
    }

    @Test
    public void anIdFromTheHostsListWinsEverywhere() throws Exception {
        for (String host : new String[] {"apollo", "vibepollo", "rubylight"}) {
            resetHost();
            FakeHost.listed = new NvApp("Game", 99, false);
            FakeHost.listed.setAppUuid(UUID);
            HttpUrl query = launch(serverInfo(host, true, false));
            assertEquals(host, "99", query.queryParameter("appid"));
            assertEquals(host, UUID, query.queryParameter("appuuid"));
        }
    }

    @Implements(value = NvHTTP.class, isInAndroidSdk = false)
    public static class FakeHost {
        static String serverInfo;
        static NvApp listed;
        static final List<String> requests = new ArrayList<>();
        static final List<HttpUrl> launches = new ArrayList<>();

        @Implementation public String getServerInfo(boolean likelyOnline) { return serverInfo; }
        @Implementation public NvApp getAppByUuid(String uuid) { return listed; }
        @Implementation public boolean quitApp() {
            requests.add("cancel");
            return true;
        }
        @Implementation public boolean launchApp(ConnectionContext context, String verb, int appId, boolean hdr)
                throws HostHttpResponseException {
            requests.add(verb);
            launches.add(HttpUrl.get("https://host/" + verb + "?" + NvHTTP.getLaunchQuery(context, appId, hdr)));
            // Stop here: the stream itself is not under test.
            throw new HostHttpResponseException(503, "Test host stops after the launch request");
        }
    }
}

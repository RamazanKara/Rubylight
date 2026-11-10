package com.limelight.nvstream;

import android.app.Application;

import com.limelight.R;
import com.limelight.nvstream.av.video.VideoDecoderRenderer;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostHttpResponseException;
import com.limelight.nvstream.http.LimelightCryptoProvider;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.jni.MoonBridge;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.annotation.RealObject;
import org.robolectric.util.ReflectionHelpers;
import org.xmlpull.v1.XmlPullParserException;

import java.io.IOException;
import java.lang.reflect.Proxy;
import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, shadows = NvConnectionRecoveryTest.FakeHost.class)
public class NvConnectionRecoveryTest {
    @Before public void resetHost() {
        FakeHost.initial = serverInfo("SUNSHINE_SERVER_BUSY", 73, "game-a", true, -1);
        FakeHost.probed = serverInfo("SUNSHINE_SERVER_FREE", 0, "", true, -1);
        FakeHost.probeFailure = null;
        FakeHost.resumeFailure = null;
        FakeHost.resumeResult = false;
        FakeHost.requests.clear();
        FakeHost.probes = 0;
        FakeHost.appLookups = 0;
    }

    @Test public void recoveryOfAnIdleHostEndsWithoutLaunchingOrFailing() throws Exception {
        FakeHost.initial = FakeHost.probed;
        Result result = start(true);
        assertGraceful(result);
        assertTrue(FakeHost.requests.isEmpty());
        assertEquals(1, FakeHost.probes);
    }

    @Test public void exitBetweenServerInfoAndResume503EndsGracefully() throws Exception {
        FakeHost.resumeFailure = new HostHttpResponseException(503, "No active session");
        assertGraceful(start(true));
        assertEquals(List.of("resume"), FakeHost.requests);
        assertEquals(1, FakeHost.probes);
    }

    @Test public void unsuccessfulResumeResponseAlsoChecksWhetherTheGameExited() throws Exception {
        assertGraceful(start(true));
        assertEquals(List.of("resume"), FakeHost.requests);
    }

    @Test public void recoveryAfterAMediaFailureCannotHideItWhenTheHostBecomesIdle() throws Exception {
        for (boolean endedBeforeResume : new boolean[] {true, false}) {
            FakeHost.initial = endedBeforeResume ? FakeHost.probed :
                    serverInfo("SUNSHINE_SERVER_BUSY", 73, "game-a", true, -1);
            FakeHost.resumeFailure = new HostHttpResponseException(503, "No active session");
            Result result = start(true, false);
            assertEquals(List.of(endedBeforeResume ? 0 : 503), result.failures);
            assertTrue(result.terminations.isEmpty());
            assertEquals(0, FakeHost.probes);
        }
    }

    @Test public void busyReplacedUnpairedDeniedOrUnknownHostNeverHidesAResume503() throws Exception {
        for (String probed : new String[] {
                FakeHost.initial,
                serverInfo("SUNSHINE_SERVER_BUSY", 99, "game-b", true, -1),
                serverInfo("SUNSHINE_SERVER_FREE", 0, "", false, -1),
                serverInfo("SUNSHINE_SERVER_FREE", 0, "", true, 0),
                serverInfo("UNRECOGNIZED", 0, "", true, -1)}) {
            FakeHost.probed = probed;
            FakeHost.resumeFailure = new HostHttpResponseException(503, "Host failure");
            Result result = start(true);
            assertEquals(List.of(503), result.failures);
            assertTrue(result.terminations.isEmpty());
        }
    }

    @Test public void unreachableOrUntrustedIdleProbeKeepsTheOriginalHostError() throws Exception {
        for (IOException failure : new IOException[] {new IOException("Offline"),
                new javax.net.ssl.SSLHandshakeException("Untrusted host")}) {
            FakeHost.probeFailure = failure;
            FakeHost.resumeFailure = new HostHttpResponseException(503, "Host failure");
            Result result = start(true);
            assertEquals(List.of(503), result.failures);
            assertTrue(result.terminations.isEmpty());
        }
    }

    @Test public void ownerPermissionMinimizedAndReplacementFailuresAreNeverSuppressed() throws Exception {
        for (int error : new int[] {401, 403, 409, 470, 525}) {
            FakeHost.resumeFailure = new HostHttpResponseException(error, "Denied");
            Result result = start(true);
            assertEquals(1, result.failures.size());
            assertTrue(result.terminations.isEmpty());
            assertEquals(0, FakeHost.probes);
        }
    }

    @Test public void anotherAppOrUuidRemainsARecoveryFailureWithoutAnyLaunchRequest() throws Exception {
        for (String initial : new String[] {
                serverInfo("SUNSHINE_SERVER_BUSY", 99, "game-b", true, -1),
                serverInfo("SUNSHINE_SERVER_BUSY", 73, "game-b", true, -1)}) {
            FakeHost.initial = initial;
            Result result = start(true);
            assertEquals(List.of(0), result.failures);
            assertTrue(result.terminations.isEmpty());
            assertTrue(FakeHost.requests.isEmpty());
            assertEquals(0, FakeHost.probes);
        }
    }

    /** A PC that offers Continue here, whose native API answers with {@code transport}. */
    private static NvConnection offering(com.limelight.nativeapi.NativeApiTransport transport) {
        NvApp app = new NvApp("Game", 73, false);
        app.setAppUuid("game-a");
        NvConnection connection = new NvConnection(RuntimeEnvironment.getApplication(),
                new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "device",
                new StreamConfiguration.Builder().setApp(app).setResumeOnly(true).setAllowIdleSessionEnd(true)
                        .setResolution(1280, 720).setRefreshRate(60)
                        .setSupportedVideoFormats(MoonBridge.VIDEO_FORMAT_H264).build(), crypto(), null) {
            @Override public boolean offersContinueHere() { return true; }
            @Override public com.limelight.nativeapi.HostEndpoint hostEndpoint() {
                return new com.limelight.nativeapi.HostEndpoint("test-host", "192.0.2.1", 47989, 47984,
                        "pem", "device", new byte[0], new byte[0]);
            }
        };
        connection.setNativeApiTransport(transport);
        return connection;
    }

    @Test public void aResumeThatCannotAskWhetherTheStreamMovedFailsAsATransportFailureARecoveryRetries() throws Exception {
        FakeHost.initial = serverInfo("SUNSHINE_SERVER_BUSY", 99, "game-b", true, -1);
        NvConnection connection = offering((host, op, argument) -> {
            throw new com.limelight.nativeapi.NativeApiException(
                    com.limelight.nativeapi.NativeApiException.Kind.NETWORK, "refused");
        });
        Result result = start(connection);
        assertEquals(List.of(0), result.failures);
        assertEquals(List.of(MoonBridge.ML_PORT_FLAG_TCP_47984 | MoonBridge.ML_PORT_FLAG_TCP_47989), result.failurePorts);
        assertEquals("the stream's end still reads as no answer", null, connection.streamMovedTo(false));
    }

    @Test public void aResumeThatAsksAndHearsNothingMovedKeepsItsVerdict() throws Exception {
        FakeHost.initial = serverInfo("SUNSHINE_SERVER_BUSY", 99, "game-b", true, -1);
        Result result = start(offering((host, op, argument) -> "{\"ok\":{}}"));
        assertEquals(List.of(0), result.failures);
        assertEquals("not a transport failure: nothing retries", List.of(0), result.failurePorts);
    }

    @Test public void aResumeThatHearsTheStreamMovedSaysSo() throws Exception {
        FakeHost.initial = serverInfo("SUNSHINE_SERVER_BUSY", 99, "game-b", true, -1);
        Result result = start(offering(
                (host, op, argument) -> "{\"ok\":{\"moved_to\":{\"device\":\"TV\",\"since_unix_ms\":5}}}"));
        assertEquals(List.of("TV"), result.movedTo);
    }

    /** A join on a PC that offers Continue here, whose native API answers with {@code transport}. */
    private static NvConnection joining(com.limelight.nativeapi.NativeApiTransport transport) {
        NvApp app = new NvApp("Game", 73, false);
        app.setAppUuid("game-a");
        NvConnection connection = new NvConnection(RuntimeEnvironment.getApplication(),
                new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "device",
                new StreamConfiguration.Builder().setApp(app).setJoinRunning(true)
                        .setResolution(1280, 720).setRefreshRate(60)
                        .setSupportedVideoFormats(MoonBridge.VIDEO_FORMAT_H264).build(), crypto(), null) {
            @Override public boolean offersContinueHere() { return true; }
            @Override public com.limelight.nativeapi.HostEndpoint hostEndpoint() {
                return new com.limelight.nativeapi.HostEndpoint("test-host", "192.0.2.1", 47989, 47984,
                        "pem", "device", new byte[0], new byte[0]);
            }
        };
        connection.setNativeApiTransport(transport);
        return connection;
    }

    /** What the PC tells a device that does not own the game. */
    private static final String HIDDEN_GAME = serverInfo("SUNSHINE_SERVER_FREE", 0, "", true, -1);

    @Test public void aJoinGetsPastTheGameThePcHidesAndOnlyResumes() throws Exception {
        FakeHost.initial = HIDDEN_GAME;
        // The note a late flip left must not bring "Now playing on" back: a join does not read it.
        Result result = start(joining((host, op, argument) -> {
            throw new AssertionError("a join reads nothing: " + op);
        }));
        assertEquals("/resume and no /launch, which could start a closed game", List.of("resume"), FakeHost.requests);
        assertEquals(0, FakeHost.appLookups);
        assertTrue(result.movedTo.isEmpty());
        assertEquals("the PC said no, loudly", List.of(0), result.failures);
        assertTrue(result.terminations.isEmpty());
        assertEquals(0, FakeHost.probes);
    }

    @Test public void aJoinThePcRefusesFailsAndStartsNothing() throws Exception {
        FakeHost.initial = HIDDEN_GAME;
        FakeHost.resumeFailure = new HostHttpResponseException(503, "no application or remote monitor to resume");
        Result result = start(joining((host, op, argument) -> "{\"ok\":{}}"));
        assertEquals(List.of("resume"), FakeHost.requests);
        assertEquals(List.of(503), result.failures);
        assertTrue(result.terminations.isEmpty());
        assertEquals("our sentence, not the host's words", List.of(sessionChanged()), result.messages);
    }

    private static String sessionChanged() {
        return RuntimeEnvironment.getApplication().getString(com.limelight.R.string.stream_recovery_session_changed);
    }

    @Test public void aJoinOfAGameThatChangedSaysSoAndStartsNothing() throws Exception {
        FakeHost.initial = HIDDEN_GAME;
        FakeHost.resumeFailure = new HostHttpResponseException(409, "The game running on this PC changed");
        Result result = start(joining((host, op, argument) -> "{\"ok\":{}}"));
        assertEquals(List.of("resume"), FakeHost.requests);
        assertEquals(List.of(409), result.failures);
        assertEquals(List.of(sessionChanged()), result.messages);
    }

    @Test public void aJoinBesideABrowserStreamSaysToEndItFirst() throws Exception {
        FakeHost.initial = HIDDEN_GAME;
        FakeHost.resumeFailure = new HostHttpResponseException(409, "Another device is streaming over WebRTC");
        Result result = start(joining((host, op, argument) -> "{\"ok\":{}}"));
        assertEquals(List.of("resume"), FakeHost.requests);
        assertEquals(List.of(409), result.failures);
        assertEquals(List.of(RuntimeEnvironment.getApplication().getString(com.limelight.R.string.continue_here_browser_stream)),
                result.messages);
    }

    @Test public void aFailureThatIsNotAboutTheGameKeepsTheHostsWords() throws Exception {
        FakeHost.initial = HIDDEN_GAME;
        FakeHost.resumeFailure = new HostHttpResponseException(503, "Encoder failure");
        Result result = start(joining((host, op, argument) -> "{\"ok\":{}}"));
        assertEquals(List.of(503), result.failures);
        assertTrue(result.messages.toString(), result.messages.get(0).contains("Encoder failure"));
    }

    @Test public void aJoinNeverOffersToReplaceTheGame() throws Exception {
        FakeHost.initial = HIDDEN_GAME;
        FakeHost.resumeFailure = new HostHttpResponseException(410,
                "An app is already running. Launch this app again within 60 seconds to confirm that you want to close it.");
        Result result = start(joining((host, op, argument) -> "{\"ok\":{}}"));
        assertEquals(List.of("resume"), FakeHost.requests);
        assertEquals(List.of(410), result.failures);
    }

    @Test public void withoutAJoinTheSameHiddenGameStillStops() throws Exception {
        FakeHost.initial = HIDDEN_GAME;
        assertGraceful(start(true));
        assertTrue(FakeHost.requests.isEmpty());
    }

    @Test public void aRejoinAfterASettingsChangeAsksForTheNewMode() throws Exception {
        // Nightjar issue #85: the game started at 1080p60 keeps running and the settings change to
        // 1600p144; the rejoin asks for the new mode, as Moonlight's /resume does.
        String[] queries = new String[2];
        int[][] modes = {{1920, 1080, 60}, {2560, 1600, 144}};
        for (int i = 0; i < modes.length; i++) {
            NvApp app = new NvApp("Game", 73, false);
            app.setAppUuid("game-a");
            start(new NvConnection(RuntimeEnvironment.getApplication(),
                    new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "device",
                    new StreamConfiguration.Builder().setApp(app)
                            // As GameConnection builds it: the launch rate is the chosen frame rate too.
                            .setResolution(modes[i][0], modes[i][1]).setRefreshRate(modes[i][2])
                            .setLaunchRefreshRate(modes[i][2])
                            .setSupportedVideoFormats(MoonBridge.VIDEO_FORMAT_H264).build(), crypto(), null));
            queries[i] = FakeHost.lastQuery;
        }
        assertEquals(List.of("resume", "resume"), FakeHost.requests);
        assertTrue(queries[0], queries[0].contains("&mode=1920x1080x60&"));
        assertTrue(queries[1], queries[1].contains("&mode=2560x1600x144&"));
    }

    @Test public void firstLaunchStillLaunchesAndIts503RemainsAnError() throws Exception {
        FakeHost.initial = FakeHost.probed;
        FakeHost.resumeFailure = new HostHttpResponseException(503, "Encoder failure");
        Result result = start(false);
        assertEquals(List.of("launch"), FakeHost.requests);
        assertEquals(List.of(503), result.failures);
        assertTrue(result.terminations.isEmpty());
        assertEquals(0, FakeHost.probes);
    }

    // Nightjar #88: a PC that took the launch and never answered held the start for good.
    @Test public void aLaunchThePcNeverAnswersEndsWithItsOwnMessageAndNoPortTest() throws Exception {
        FakeHost.initial = FakeHost.probed;
        FakeHost.resumeFailure = new NvHTTP.LaunchTimeoutException(NvHTTP.LAUNCH_READ_TIMEOUT_SECONDS);
        Result result = start(false);
        assertEquals(List.of("launch"), FakeHost.requests);
        assertEquals(List.of(0), result.failures);
        assertEquals("not a blocked port", List.of(0), result.failurePorts);
        assertEquals(List.of(RuntimeEnvironment.getApplication().getString(R.string.startup_launch_timeout,
                NvHTTP.LAUNCH_READ_TIMEOUT_SECONDS)), result.messages);
    }

    // Nightjar #88: a start behind a previous connection whose stop never ends waited for it without a limit.
    @Test public void aStartBehindAConnectionThatNeverEndsSaysSoInsteadOfWaiting() throws Exception {
        java.lang.reflect.Field field = NvConnection.class.getDeclaredField("connectionAllowed");
        field.setAccessible(true);
        java.util.concurrent.Semaphore semaphore = (java.util.concurrent.Semaphore) field.get(null);
        long wait = NvConnection.connectionWaitMs;
        assertTrue(semaphore.tryAcquire());
        NvConnection.connectionWaitMs = 50;
        try {
            Result result = start(false);
            assertTrue("nothing reached the PC", FakeHost.requests.isEmpty());
            assertEquals(List.of(0), result.failures);
            assertEquals(List.of(RuntimeEnvironment.getApplication().getString(R.string.stream_stop_timeout)),
                    result.messages);
            assertEquals("the permit is not this start's to release", 0, semaphore.availablePermits());
        } finally {
            NvConnection.connectionWaitMs = wait;
            semaphore.release();
        }
    }

    @Test public void aRecoveryProbeCancelledDuringTeardownCannotFinishTheSession() throws Exception {
        NvConnection connection = connection(true);
        NvHTTP http = new NvHTTP(new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984,
                "device", null, crypto()) {
            @Override public ComputerDetails probePairedComputer() throws IOException, XmlPullParserException {
                connection.stop();
                return getComputerDetails(FakeHost.probed);
            }
        };
        ReflectionHelpers.setField(connection, "http", http);
        assertFalse(connection.hasEndedSession());
        assertFalse(connection.hasEndedSession());
    }

    @Test public void apolloInputOnlyModeIsNotAnEndedSession() throws Exception {
        // Apollo reports game 0 and a free state while its input-only mode hides the game, but keeps its UUID.
        NvConnection connection = connection(true);
        ReflectionHelpers.setField(connection, "http", new NvHTTP(new ComputerDetails.AddressTuple("192.0.2.1", 47989),
                47984, "device", null, crypto()));
        FakeHost.probed = serverInfo("SUNSHINE_SERVER_FREE", 0, "game-a", true, -1);
        assertFalse(connection.hasEndedSession());
        FakeHost.probed = serverInfo("SUNSHINE_SERVER_FREE", 0, "", true, -1);
        assertTrue(connection.hasEndedSession());
    }

    @Test public void recoveryIntoApolloInputOnlyModeDoesNotEndTheStream() throws Exception {
        FakeHost.initial = serverInfo("SUNSHINE_SERVER_FREE", 0, "game-a", true, -1);
        FakeHost.probed = FakeHost.initial;
        Result result = start(true);
        assertTrue(result.terminations.isEmpty());
        assertEquals(1, result.failures.size());
    }

    @Test public void onlyAnExplicitFreeHostStateConfirmsAnIdleSession() throws Exception {
        NvHTTP http = new NvHTTP(new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984,
                "device", null, crypto());
        for (String state : new String[] {"SUNSHINE_SERVER_FREE", "MJOLNIR_SERVER_FREE", "UNKNOWN"}) {
            ComputerDetails details = http.getComputerDetails(serverInfo(state, 73, "game-a", true, -1));
            assertEquals(0, details.runningGameId);
            assertEquals(state.endsWith("_SERVER_FREE"), details.sessionIdle);
        }
        assertThrows(XmlPullParserException.class, () ->
                http.getComputerDetails(serverInfo("", 0, "", true, -1)));
    }

    private static NvConnection connection(boolean recovery) {
        return connection(recovery, true);
    }

    private static NvConnection connection(boolean recovery, boolean allowIdleSessionEnd) {
        NvApp app = new NvApp("Game", 73, false);
        app.setAppUuid("game-a");
        return new NvConnection(RuntimeEnvironment.getApplication(),
                new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "device",
                new StreamConfiguration.Builder().setApp(app).setResumeOnly(recovery)
                        .setAllowIdleSessionEnd(allowIdleSessionEnd)
                        .setResolution(1280, 720).setRefreshRate(60)
                        .setSupportedVideoFormats(MoonBridge.VIDEO_FORMAT_H264).build(), crypto(), null);
    }

    private static LimelightCryptoProvider crypto() {
        return new LimelightCryptoProvider() {
            @Override public java.security.cert.X509Certificate getClientCertificate() { return null; }
            @Override public java.security.PrivateKey getClientPrivateKey() { return null; }
            @Override public byte[] getPemEncodedClientCertificate() { return new byte[0]; }
            @Override public String encodeBase64String(byte[] data) { return ""; }
        };
    }

    private static Result start(boolean recovery) throws Exception {
        return start(recovery, true);
    }

    private static Result start(boolean recovery, boolean allowIdleSessionEnd) throws Exception {
        return start(connection(recovery, allowIdleSessionEnd));
    }

    private static Result start(NvConnection connection) throws Exception {
        Result result = new Result();
        NvConnectionListener listener = (NvConnectionListener) Proxy.newProxyInstance(
                NvConnectionListener.class.getClassLoader(), new Class<?>[] {NvConnectionListener.class},
                (proxy, method, args) -> {
                    if (method.getName().equals("stageFailed")) {
                        result.failurePorts.add((Integer) args[1]);
                        result.failures.add((Integer) args[2]);
                    } else if (method.getName().equals("streamMoved")) result.movedTo.add((String) args[0]);
                    else if (method.getName().equals("connectionTerminated")) result.terminations.add((Integer) args[0]);
                    else if (method.getName().equals("displayMessage")) result.messages.add((String) args[0]);
                    return null;
                });
        connection.start(null, new VideoDecoderRenderer() {
            @Override public int setup(int format, int width, int height, int rate) {
                throw new AssertionError("Ended session initialized native video");
            }
            @Override public void start() { fail("Ended session started video"); }
            @Override public void stop() {}
            @Override public void cleanup() { result.cleanups++; }
            @Override public int getCapabilities() { return 0; }
            @Override public void setHdrMode(boolean enabled, byte[] metadata) {}
            @Override public int submitDecodeUnit(byte[] data, int length, int type, int frame, int frameType,
                    char latency, long received, long queued, long receivedNs, long presentationUs) { return 0; }
        }, listener);
        Thread worker = ConnectionStartup.await(connection);
        try {
            assertFalse("Connection startup did not finish", worker.isAlive());
        } finally {
            if (worker.isAlive()) {
                connection.stop();
                worker.join(5000);
            }
        }
        assertEquals(1, result.cleanups);
        return result;
    }

    private static void assertGraceful(Result result) {
        assertEquals(List.of(MoonBridge.ML_ERROR_GRACEFUL_TERMINATION), result.terminations);
        assertTrue(result.failures.isEmpty());
    }

    private static class Result {
        final List<Integer> failures = new ArrayList<>();
        final List<Integer> failurePorts = new ArrayList<>();
        final List<String> movedTo = new ArrayList<>();
        final List<Integer> terminations = new ArrayList<>();
        final List<String> messages = new ArrayList<>();
        int cleanups;
    }

    private static String serverInfo(String state, int id, String uuid, boolean paired, long permission) {
        return "<root status_code=\"200\"><appversion>7.1.431.0</appversion><uniqueid>test-host</uniqueid>" +
                "<HttpsPort>47984</HttpsPort><state>" + state + "</state><currentgame>" + id + "</currentgame>" +
                "<currentgameuuid>" + uuid + "</currentgameuuid><PairStatus>" + (paired ? 1 : 0) + "</PairStatus>" +
                (permission < 0 ? "" : "<Permission>" + permission + "</Permission>") + "</root>";
    }

    @Implements(value = NvHTTP.class, isInAndroidSdk = false)
    public static class FakeHost {
        @RealObject NvHTTP http;
        static String initial, probed;
        static IOException probeFailure, resumeFailure;
        static boolean resumeResult;
        // Whether the last launch or resume asked for HDR.
        static boolean lastHdr;
        // The query the last launch or resume would send.
        static String lastQuery;
        static int probes;
        static int appLookups;
        static final List<String> requests = new ArrayList<>();
        // The link test before a launch: its result in Mbit/s, or the failure it throws.
        static double linkMbps;
        static IOException linkFailure;

        @Implementation public String getServerInfo(boolean likelyOnline) { return initial; }
        @Implementation public double probeLinkMbps() throws IOException {
            requests.add("bandwidth-probe");
            if (linkFailure != null) throw linkFailure;
            return linkMbps;
        }
        @Implementation public NvApp getAppByUuid(String uuid) {
            appLookups++;
            NvApp app = new NvApp("Game", 73, false);
            app.setAppUuid(uuid);
            return app;
        }
        @Implementation public ComputerDetails probePairedComputer() throws IOException, XmlPullParserException {
            probes++;
            if (probeFailure != null) throw probeFailure;
            return http.getComputerDetails(probed);
        }
        @Implementation public boolean launchApp(ConnectionContext context, String verb, int id, boolean hdr)
                throws IOException {
            requests.add(verb);
            lastHdr = hdr;
            try {
                java.lang.reflect.Method query = NvHTTP.class.getDeclaredMethod("getLaunchQuery",
                        ConnectionContext.class, int.class, boolean.class);
                query.setAccessible(true);
                lastQuery = (String) query.invoke(null, context, id, hdr);
            } catch (ReflectiveOperationException e) {
                throw new AssertionError(e);
            }
            if (resumeFailure != null) throw resumeFailure;
            return resumeResult;
        }
        @Implementation public boolean quitApp() { throw new AssertionError("Recovery quit an app"); }
    }
}

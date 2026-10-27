package com.limelight.nvstream;

import android.app.Application;
import android.content.SharedPreferences;
import android.preference.PreferenceManager;

import com.limelight.nvstream.av.video.VideoDecoderRenderer;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.FecChoice;
import com.limelight.nvstream.http.LimelightCryptoProvider;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.io.IOException;
import java.lang.reflect.Proxy;
import java.util.List;

import static org.junit.Assert.*;

/** The link test before a new stream's first bitrate, and the error correction asked for at launch. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = {NvConnectionRecoveryTest.FakeHost.class, NvConnectionHapticsFlagTest.CapturingBridge.class,
                NvConnectionHapticsFlagTest.FakeWiredLink.class},
        instrumentedPackages = {"com.limelight.nvstream.jni", "com.limelight.utils"})
public class NvConnectionFirstBitrateTest {
    private static final String NEW = "<state>SUNSHINE_SERVER_FREE</state><currentgame>0</currentgame><currentgameuuid></currentgameuuid>";
    private static final String RUNNING = "<state>SUNSHINE_SERVER_BUSY</state><currentgame>73</currentgame><currentgameuuid>game-a</currentgameuuid>";
    private static final String PROBE = "<BandwidthProbeVersion>1</BandwidthProbeVersion>";
    private static final String FEC = "<FecControlVersion>1</FecControlVersion>";

    @Before public void reset() {
        NvConnectionHapticsFlagTest.FakeWiredLink.mbps = 0;
        NvConnectionHapticsFlagTest.FakeWiredLink.wireless = false;
        NvConnectionRecoveryTest.FakeHost.linkMbps = 0;
        NvConnectionRecoveryTest.FakeHost.linkFailure = null;
        NvConnectionRecoveryTest.FakeHost.lastQuery = null;
    }

    @Test public void aNewStreamAtTheAutomaticBitrateStartsAt80PercentOfTheLinkTest() throws Exception {
        NvConnectionRecoveryTest.FakeHost.linkMbps = 25;
        NvConnection connection = start(NEW + PROBE, 40000, true, null);
        assertEquals(List.of("bandwidth-probe", "launch"), NvConnectionRecoveryTest.FakeHost.requests);
        assertEquals(20000, NvConnectionHapticsFlagTest.CapturingBridge.bitrate);
        assertEquals(20000, connection.getStartBitrateKbps());
    }

    @Test public void aFastLinkNeverRaisesTheBitrate() throws Exception {
        NvConnectionRecoveryTest.FakeHost.linkMbps = 900;
        start(NEW + PROBE, 40000, true, null);
        assertEquals(40000, NvConnectionHapticsFlagTest.CapturingBridge.bitrate);
    }

    // A failed test is logged and the stream still launches, at the configured bitrate.
    @Test public void aFailedLinkTestStartsAtTheConfiguredBitrate() throws Exception {
        NvConnectionRecoveryTest.FakeHost.linkFailure = new IOException("409 Conflict");
        start(NEW + PROBE, 40000, true, null);
        assertEquals(List.of("bandwidth-probe", "launch"), NvConnectionRecoveryTest.FakeHost.requests);
        assertEquals(40000, NvConnectionHapticsFlagTest.CapturingBridge.bitrate);
    }

    // The link test measured worse on the NUC (rust/PERFORMANCE.md), so it runs only when switched
    // on: by default a new stream at the automatic bitrate on a PC with the test is not tested.
    @Test public void theLinkTestIsOffByDefaultAndRunsOnlyWhenSwitchedOn() throws Exception {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication());
        prefs.edit().clear().commit();
        NvConnectionRecoveryTest.FakeHost.linkMbps = 25;
        PreferenceConfiguration defaults = PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication());
        assertTrue(defaults.automaticBitrate);
        assertFalse(defaults.linkProbe);
        assertFalse(defaults.probesFirstBitrate());
        start(NEW + PROBE, 40000, defaults.probesFirstBitrate(), null);
        assertEquals(List.of("launch"), NvConnectionRecoveryTest.FakeHost.requests);
        assertEquals(40000, NvConnectionHapticsFlagTest.CapturingBridge.bitrate);

        prefs.edit().putBoolean("checkbox_link_probe", true).commit();
        PreferenceConfiguration on = PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication());
        assertTrue(on.probesFirstBitrate());
        start(NEW + PROBE, 40000, on.probesFirstBitrate(), null);
        assertEquals(List.of("bandwidth-probe", "launch"), NvConnectionRecoveryTest.FakeHost.requests);
        assertEquals(20000, NvConnectionHapticsFlagTest.CapturingBridge.bitrate);

        // A bitrate the user set is never tested, switched on or not.
        prefs.edit().putInt("seekbar_bitrate_kbps", 40000).putBoolean("bitrate_user_set", true).commit();
        assertFalse(PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication()).probesFirstBitrate());
        prefs.edit().clear().commit();
    }

    // A bitrate the user chose, a PC without the test and a resume (the PC refuses the test while
    // it streams) all start at the configured bitrate without testing.
    @Test public void onlyANewStreamAtTheAutomaticBitrateOnAPcWithTheTestIsTested() throws Exception {
        NvConnectionRecoveryTest.FakeHost.linkMbps = 5;
        start(NEW + PROBE, 40000, false, null);
        assertEquals(List.of("launch"), NvConnectionRecoveryTest.FakeHost.requests);
        assertEquals(40000, NvConnectionHapticsFlagTest.CapturingBridge.bitrate);

        start(NEW, 40000, true, null);
        assertEquals(List.of("launch"), NvConnectionRecoveryTest.FakeHost.requests);
        assertEquals(40000, NvConnectionHapticsFlagTest.CapturingBridge.bitrate);

        start(RUNNING + PROBE, 40000, true, null);
        assertEquals(List.of("resume"), NvConnectionRecoveryTest.FakeHost.requests);
        assertEquals(40000, NvConnectionHapticsFlagTest.CapturingBridge.bitrate);
    }

    // The setting reaches the launch of a PC that takes it; one without FecControlVersion is not asked.
    @Test public void theErrorCorrectionSettingReachesTheLaunch() throws Exception {
        start(NEW + FEC, 20000, false, FecChoice.AUTO);
        assertTrue(NvConnectionRecoveryTest.FakeHost.lastQuery, NvConnectionRecoveryTest.FakeHost.lastQuery
                .endsWith("&" + FecChoice.LAUNCH_PARAMETER + "=auto"));
        start(NEW + FEC, 20000, false, FecChoice.fixed(30));
        assertTrue(NvConnectionRecoveryTest.FakeHost.lastQuery.endsWith("&" + FecChoice.LAUNCH_PARAMETER + "=30"));
        start(NEW, 20000, false, FecChoice.AUTO);
        assertFalse(NvConnectionRecoveryTest.FakeHost.lastQuery.contains(FecChoice.LAUNCH_PARAMETER));
    }

    private static NvConnection start(String serverInfo, int bitrate, boolean automatic, FecChoice fec) throws Exception {
        NvConnectionRecoveryTest.FakeHost.initial = "<root status_code=\"200\"><appversion>7.1.431.0</appversion>" +
                "<uniqueid>test-host</uniqueid><HttpsPort>47984</HttpsPort><PairStatus>1</PairStatus>" +
                "<RustHostVersion>3.0.0</RustHostVersion>" + serverInfo + "</root>";
        NvConnectionRecoveryTest.FakeHost.resumeFailure = null;
        NvConnectionRecoveryTest.FakeHost.resumeResult = true;
        NvConnectionRecoveryTest.FakeHost.requests.clear();
        NvConnectionHapticsFlagTest.CapturingBridge.bitrate = -1;
        NvApp app = new NvApp("Game", 73, false);
        app.setAppUuid("game-a");
        NvConnection connection = new NvConnection(RuntimeEnvironment.getApplication(),
                new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "device",
                new StreamConfiguration.Builder().setApp(app).setResolution(1280, 720).setRefreshRate(60)
                        .setBitrate(bitrate).setProbeFirstBitrate(automatic).setFecChoice(fec)
                        .setSupportedVideoFormats(MoonBridge.VIDEO_FORMAT_H264).build(), crypto(), null);
        NvConnectionListener listener = (NvConnectionListener) Proxy.newProxyInstance(
                NvConnectionListener.class.getClassLoader(), new Class<?>[] {NvConnectionListener.class},
                (proxy, method, args) -> null);
        connection.start(null, renderer(), listener);
        Thread worker = ConnectionStartup.await(connection);
        assertFalse("Connection startup did not finish", worker.isAlive());
        return connection;
    }

    private static VideoDecoderRenderer renderer() {
        return new VideoDecoderRenderer() {
            @Override public int setup(int format, int width, int height, int rate) { return -1; }
            @Override public void start() {}
            @Override public void stop() {}
            @Override public void cleanup() {}
            @Override public int getCapabilities() { return 0; }
            @Override public void setHdrMode(boolean enabled, byte[] metadata) {}
            @Override public int submitDecodeUnit(byte[] data, int length, int type, int frame, int frameType,
                    char latency, long received, long queued, long receivedNs, long presentationUs) { return 0; }
        };
    }

    private static LimelightCryptoProvider crypto() {
        return new LimelightCryptoProvider() {
            @Override public java.security.cert.X509Certificate getClientCertificate() { return null; }
            @Override public java.security.PrivateKey getClientPrivateKey() { return null; }
            @Override public byte[] getPemEncodedClientCertificate() { return new byte[0]; }
            @Override public String encodeBase64String(byte[] data) { return ""; }
        };
    }
}

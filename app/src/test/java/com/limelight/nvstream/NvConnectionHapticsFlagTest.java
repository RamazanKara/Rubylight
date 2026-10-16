package com.limelight.nvstream;

import android.app.Application;
import android.content.SharedPreferences;
import android.preference.PreferenceManager;

import com.limelight.nvstream.av.video.VideoDecoderRenderer;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.LimelightCryptoProvider;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.util.ReflectionHelpers;

import java.lang.reflect.Proxy;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = {NvConnectionRecoveryTest.FakeHost.class, NvConnectionHapticsFlagTest.CapturingBridge.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class NvConnectionHapticsFlagTest {
    // Haptics PCM switches a Vibepollo host's control loop to 5 ms; only a USB DualSense with
    // waveform haptics enabled can play it.
    @Test public void onlyUsbDualSenseHapticsAsksTheHostForHapticsPcm() throws Exception {
        boolean[][] settings = {{false, false}, {true, false}, {false, true}, {true, true}};
        for (boolean[] setting : settings) {
            SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication());
            prefs.edit().putBoolean("checkbox_usb_dualsense", setting[0])
                    .putBoolean("checkbox_usb_dualsense_haptics", setting[1]).commit();
            CapturingBridge.hapticsPcm = null;
            startStream();
            assertEquals(setting[0] && setting[1], CapturingBridge.hapticsPcm);
        }
    }

    // Only a PyroWave stream looks up the wired link; any other codec sends no pacing hints.
    @Test public void otherCodecsSendNoPyroWavePacingHints() throws Exception {
        CapturingBridge.linkMbps = CapturingBridge.paceMbps = -1;
        startStream();
        assertEquals(0, CapturingBridge.linkMbps);
        assertEquals(0, CapturingBridge.paceMbps);
    }

    // The drop tolerance reaches the native control timeout only when it is longer than it.
    @Test public void networkDropToleranceIsSetBeforeEveryStart() throws Exception {
        String[][] settings = {{"0", "0"}, {"30", "30000"}, {"60", "60000"}, {"3600", "0"}};
        for (String[] setting : settings) {
            PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit()
                    .putString("list_network_grace", setting[0]).commit();
            CapturingBridge.gracePeriodMs = null;
            startStream();
            assertEquals(Integer.valueOf(setting[1]), CapturingBridge.gracePeriodMs);
        }
    }

    private static void startStream() throws Exception {
        NvConnectionRecoveryTest.FakeHost.initial = "<root status_code=\"200\"><appversion>7.1.431.0</appversion>" +
                "<uniqueid>test-host</uniqueid><HttpsPort>47984</HttpsPort><state>SUNSHINE_SERVER_FREE</state>" +
                "<currentgame>0</currentgame><currentgameuuid></currentgameuuid><PairStatus>1</PairStatus></root>";
        NvConnectionRecoveryTest.FakeHost.resumeFailure = null;
        NvConnectionRecoveryTest.FakeHost.resumeResult = true;
        NvConnectionRecoveryTest.FakeHost.requests.clear();
        NvApp app = new NvApp("Game", 73, false);
        app.setAppUuid("game-a");
        NvConnection connection = new NvConnection(RuntimeEnvironment.getApplication(),
                new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "device",
                new StreamConfiguration.Builder().setApp(app).setResolution(1280, 720).setRefreshRate(60)
                        .setSupportedVideoFormats(MoonBridge.VIDEO_FORMAT_H264).build(), crypto(), null);
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
        assertFalse("Connection startup did not finish", worker.isAlive());
    }

    private static LimelightCryptoProvider crypto() {
        return new LimelightCryptoProvider() {
            @Override public java.security.cert.X509Certificate getClientCertificate() { return null; }
            @Override public java.security.PrivateKey getClientPrivateKey() { return null; }
            @Override public byte[] getPemEncodedClientCertificate() { return new byte[0]; }
            @Override public String encodeBase64String(byte[] data) { return ""; }
        };
    }

    @Implements(value = MoonBridge.class, isInAndroidSdk = false)
    public static class CapturingBridge {
        static Boolean hapticsPcm;
        static int serverCodecModeSupport, hostFeatures;
        static int linkMbps = -1, paceMbps = -1;
        static Integer gracePeriodMs;

        @Implementation protected static void __staticInitializer__() {}

        @Implementation
        public static void setNetworkGracePeriodMs(int gracePeriodMs) {
            CapturingBridge.gracePeriodMs = gracePeriodMs;
        }

        @Implementation
        public static int startConnection(String address, String appVersion, String gfeVersion,
                                          String rtspSessionUrl, int serverCodecModeSupport,
                                          int width, int height, int fps,
                                          int bitrate, int packetSize, int streamingRemotely,
                                          int audioConfiguration, int supportedVideoFormats,
                                          int clientRefreshRateX100,
                                          byte[] riAesKey, byte[] riAesIv,
                                          int videoCapabilities,
                                          int colorSpace, int colorRange,
                                          boolean unbatchedInput, boolean networkPriority, int hostFeatures,
                                          boolean inputOnly, boolean remoteMonitor, boolean enableMic,
                                          boolean hapticsPcm, int pyrowaveLinkMbps, int pyrowavePaceMbps) {
            CapturingBridge.hapticsPcm = hapticsPcm;
            CapturingBridge.serverCodecModeSupport = serverCodecModeSupport;
            CapturingBridge.hostFeatures = hostFeatures;
            CapturingBridge.linkMbps = pyrowaveLinkMbps;
            CapturingBridge.paceMbps = pyrowavePaceMbps;
            // Stop here: no native stream exists in this test.
            return -1;
        }
    }
}

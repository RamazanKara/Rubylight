package com.limelight.nvstream;

import android.app.Application;

import com.limelight.R;
import com.limelight.nvstream.av.video.VideoDecoderRenderer;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.LimelightCryptoProvider;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.util.ReflectionHelpers;

import java.lang.reflect.Proxy;
import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.*;

/** Input-only on a host without its own mode: an ordinary small stream that never replaces a game. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = {NvConnectionRecoveryTest.FakeHost.class, NvConnectionSmallStreamTest.CapturingBridge.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class NvConnectionSmallStreamTest {
    private static final String IDLE = "<root status_code=\"200\"><appversion>7.1.431.0</appversion>" +
            "<uniqueid>test-host</uniqueid><HttpsPort>47984</HttpsPort><state>SUNSHINE_SERVER_FREE</state>" +
            "<currentgame>0</currentgame><currentgameuuid></currentgameuuid><PairStatus>1</PairStatus></root>";
    private static final String OTHER_GAME = "<root status_code=\"200\"><appversion>7.1.431.0</appversion>" +
            "<uniqueid>test-host</uniqueid><HttpsPort>47984</HttpsPort><state>SUNSHINE_SERVER_BUSY</state>" +
            "<currentgame>99</currentgame><currentgameuuid></currentgameuuid><PairStatus>1</PairStatus></root>";

    private final List<String> messages = new ArrayList<>();

    @Before
    public void reset() {
        NvConnectionRecoveryTest.FakeHost.resumeFailure = null;
        NvConnectionRecoveryTest.FakeHost.resumeResult = true;
        NvConnectionRecoveryTest.FakeHost.requests.clear();
        CapturingBridge.started = false;
    }

    @Test public void theHostGetsAnOrdinarySmallStreamThatThisDeviceDoesNotDecode() throws Exception {
        start(IDLE, true);
        assertEquals(java.util.Collections.singletonList("launch"), NvConnectionRecoveryTest.FakeHost.requests);
        assertTrue(CapturingBridge.started);
        // The core starts video and audio as for any stream; only the host's own mode skips them.
        assertFalse(CapturingBridge.inputOnly);
        assertEquals(640, CapturingBridge.width);
        assertEquals(360, CapturingBridge.height);
        assertEquals(30, CapturingBridge.fps);
        assertEquals(500, CapturingBridge.bitrate);
        assertEquals(MoonBridge.VIDEO_FORMAT_H264, CapturingBridge.videoFormats);
    }

    @Test public void anotherRunningGameIsNeverQuit() throws Exception {
        start(OTHER_GAME, true);
        assertTrue(NvConnectionRecoveryTest.FakeHost.requests.isEmpty());
        assertFalse(CapturingBridge.started);
        assertEquals(java.util.Collections.singletonList(
                RuntimeEnvironment.getApplication().getString(R.string.input_only_other_game)), messages);
    }

    @Test public void anOrdinaryStreamIsUnchanged() throws Exception {
        start(IDLE, false);
        assertTrue(CapturingBridge.started);
        assertFalse(CapturingBridge.inputOnly);
        assertEquals(1280, CapturingBridge.width);
        assertEquals(720, CapturingBridge.height);
    }

    private void start(String serverInfo, boolean smallStream) throws Exception {
        NvConnectionRecoveryTest.FakeHost.initial = serverInfo;
        NvApp app = new NvApp("Game", 73, false);
        app.setSmallStreamInputOnly(smallStream);
        StreamConfiguration.Builder config = new StreamConfiguration.Builder().setApp(app)
                .setSupportedVideoFormats(MoonBridge.VIDEO_FORMAT_H264);
        if (smallStream) {
            config.setResolution(640, 360).setRefreshRate(30).setLaunchRefreshRate(30).setBitrate(500);
        } else {
            config.setResolution(1280, 720).setRefreshRate(60).setBitrate(20000);
        }
        NvConnection connection = new NvConnection(RuntimeEnvironment.getApplication(),
                new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "device", config.build(), crypto(), null);
        NvConnectionListener listener = (NvConnectionListener) Proxy.newProxyInstance(
                NvConnectionListener.class.getClassLoader(), new Class<?>[] {NvConnectionListener.class},
                (proxy, method, args) -> {
                    if (method.getName().equals("displayMessage")) {
                        synchronized (messages) {
                            messages.add((String) args[0]);
                        }
                    }
                    return null;
                });
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
        static volatile boolean started, inputOnly;
        static volatile int width, height, fps, bitrate, videoFormats;

        @Implementation protected static void __staticInitializer__() {}

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
                                          boolean hapticsPcm, int pyrowaveLinkMbps, int pyrowavePaceMbps,
                                          boolean clientWireless) {
            CapturingBridge.width = width;
            CapturingBridge.height = height;
            CapturingBridge.fps = fps;
            CapturingBridge.bitrate = bitrate;
            CapturingBridge.videoFormats = supportedVideoFormats;
            CapturingBridge.inputOnly = inputOnly;
            CapturingBridge.started = true;
            // Stop here: no native stream exists in this test.
            return -1;
        }
    }
}

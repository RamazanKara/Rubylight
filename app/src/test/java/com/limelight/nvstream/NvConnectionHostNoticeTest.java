package com.limelight.nvstream;

import android.app.Application;

import com.limelight.R;
import com.limelight.nvstream.av.video.VideoDecoderRenderer;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.LimelightCryptoProvider;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.TestHosts;
import com.limelight.nvstream.jni.MoonBridge;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.lang.reflect.Proxy;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = {NvConnectionRecoveryTest.FakeHost.class, NvConnectionHapticsFlagTest.CapturingBridge.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class NvConnectionHostNoticeTest {
    private static final String SUNSHINE = TestHosts.serverInfo("sunshine-serverinfo.xml");
    // The same PC after installing a host with a virtual display.
    private static final String WITH_DISPLAY = SUNSHINE.replace("</root>",
            "<Permission>4294967295</Permission><VirtualDisplayCapable>true</VirtualDisplayCapable></root>");

    private static NvConnection lastConnection;
    // What the stream's configuration says about packed 4:4:4 (Game's per-session choice).
    private static boolean sessionPacked;
    // What the launch told the renderer about the PC's packed modes, null when it said nothing.
    private static Integer hostPackedModesSeen;

    private static int negotiatedFormats() {
        ConnectionContext context = ReflectionHelpers.getField(lastConnection, "context");
        return context.negotiatedVideoFormats;
    }

    private static List<String> stream(String serverInfo, boolean virtualDisplay) throws Exception {
        return stream(serverInfo, virtualDisplay, MoonBridge.VIDEO_FORMAT_H264, false);
    }

    private static List<String> stream(String serverInfo, boolean virtualDisplay, int formats, boolean sdr10)
            throws Exception {
        return stream(serverInfo, virtualDisplay, formats, sdr10, null);
    }

    private static List<String> stream(String serverInfo, boolean virtualDisplay, int formats, boolean sdr10,
                                       String packedHdrDrop) throws Exception {
        NvConnectionRecoveryTest.FakeHost.initial = serverInfo;
        NvConnectionRecoveryTest.FakeHost.resumeFailure = null;
        NvConnectionRecoveryTest.FakeHost.resumeResult = true;
        NvConnectionRecoveryTest.FakeHost.requests.clear();
        NvApp app = new NvApp("Game", 73, false);
        NvConnection connection = new NvConnection(RuntimeEnvironment.getApplication(),
                new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "device",
                new StreamConfiguration.Builder().setApp(app).setResolution(1280, 720).setRefreshRate(60)
                        .setVirtualDisplay(virtualDisplay, 100)
                        .setSupportedVideoFormats(formats).setSdr10Bit(sdr10).setPacked444(sessionPacked).build(),
                crypto(), null);
        List<String> messages = Collections.synchronizedList(new ArrayList<>());
        NvConnectionListener listener = (NvConnectionListener) Proxy.newProxyInstance(
                NvConnectionListener.class.getClassLoader(), new Class<?>[] {NvConnectionListener.class},
                (proxy, method, args) -> {
                    if (method.getName().equals("displayTransientMessage")) messages.add((String) args[0]);
                    return null;
                });
        lastConnection = connection;
        hostPackedModesSeen = null;
        connection.start(null, new VideoDecoderRenderer() {
            @Override public void setHostPacked444Modes(int modes) { hostPackedModesSeen = modes; }
            @Override public int setup(int format, int width, int height, int rate) { return -1; }
            @Override public void start() {}
            @Override public void stop() {}
            @Override public void cleanup() {}
            @Override public int getCapabilities() { return 0; }
            @Override public String packedHdrDropReason() { return packedHdrDrop; }
            @Override public void setHdrMode(boolean enabled, byte[] metadata) {}
            @Override public int submitDecodeUnit(byte[] data, int length, int type, int frame, int frameType,
                    char latency, long received, long queued, long receivedNs, long presentationUs) { return 0; }
        }, listener);
        Thread worker = ReflectionHelpers.getField(connection, "connectionThread");
        worker.join(5000);
        assertFalse("Connection startup did not finish", worker.isAlive());
        return messages;
    }

    private static LimelightCryptoProvider crypto() {
        return new LimelightCryptoProvider() {
            @Override public java.security.cert.X509Certificate getClientCertificate() { return null; }
            @Override public java.security.PrivateKey getClientPrivateKey() { return null; }
            @Override public byte[] getPemEncodedClientCertificate() { return new byte[0]; }
            @Override public String encodeBase64String(byte[] data) { return ""; }
        };
    }

    @Test
    public void aPcWithoutAVirtualDisplayIsToldOnceNotEveryStream() throws Exception {
        String unavailable = RuntimeEnvironment.getApplication().getString(R.string.startup_virtual_display_unavailable);
        assertEquals(Collections.emptyList(), stream(SUNSHINE, false));
        assertEquals(Collections.singletonList(unavailable), stream(SUNSHINE, true));
        assertEquals(Collections.emptyList(), stream(SUNSHINE, true));
        assertEquals(Collections.emptyList(), stream(SUNSHINE, true));
        // Gaining a virtual display re-arms the notice, so losing it again is reported.
        assertEquals(Collections.emptyList(), stream(WITH_DISPLAY, true));
        assertEquals(Collections.singletonList(unavailable), stream(SUNSHINE, true));
    }

    @Test
    public void tenBitSdrNeverAsksForHdrAndAPcThatStaysAt8BitIsToldOnce() throws Exception {
        int offered = MoonBridge.VIDEO_FORMAT_H264 | MoonBridge.VIDEO_FORMAT_H265 | MoonBridge.VIDEO_FORMAT_H265_MAIN10;
        String notAccepted = RuntimeEnvironment.getApplication().getString(R.string.startup_sdr10_not_accepted);
        String hdrUnavailable = RuntimeEnvironment.getApplication().getString(R.string.startup_hdr_unavailable);
        // The PC takes 10-bit: no message, and the launch is not an HDR launch but says hdrMode=0.
        assertEquals(Collections.emptyList(), stream(SUNSHINE, false, offered, true));
        assertFalse(NvConnectionRecoveryTest.FakeHost.lastHdr);
        assertTrue(NvConnectionRecoveryTest.FakeHost.lastQuery, hdrMode(NvConnectionRecoveryTest.FakeHost.lastQuery, "0"));
        // The same offer as HDR does ask for it.
        assertEquals(Collections.emptyList(), stream(SUNSHINE, false, offered, false));
        assertTrue(NvConnectionRecoveryTest.FakeHost.lastHdr);
        assertTrue(hdrMode(NvConnectionRecoveryTest.FakeHost.lastQuery, "1"));
        // A PC with no 10-bit codec streams 8-bit: said once, as 10-bit and not as HDR.
        String eightBit = SUNSHINE.replace("<ServerCodecModeSupport>197377", "<ServerCodecModeSupport>257");
        assertEquals(Collections.singletonList(notAccepted), stream(eightBit, false, offered, true));
        assertFalse(NvConnectionRecoveryTest.FakeHost.lastHdr);
        // ...and a plain 8-bit stream sends no hdrMode at all, as before.
        assertFalse(NvConnectionRecoveryTest.FakeHost.lastQuery, NvConnectionRecoveryTest.FakeHost.lastQuery.contains("hdrMode"));
        assertEquals(Collections.emptyList(), stream(eightBit, false, offered, true));
        // Gaining 10-bit re-arms the notice.
        assertEquals(Collections.emptyList(), stream(SUNSHINE, false, offered, true));
        assertEquals(Collections.singletonList(notAccepted), stream(eightBit, false, offered, true));
        assertNotEquals(notAccepted, hdrUnavailable);
    }

    @Test
    public void theNativeStreamAsksForPackedChromaFromTheSessionNotTheSavedSetting() throws Exception {
        android.content.SharedPreferences prefs = android.preference.PreferenceManager.getDefaultSharedPreferences(
                RuntimeEnvironment.getApplication());
        try {
            for (boolean saved : new boolean[] {true, false}) {
                assertTrue(prefs.edit().putBoolean("experimental_packed_444", saved).commit());
                // A session reconnected without packed chroma: the saved setting is still on, the stream is not.
                sessionPacked = false;
                stream(SUNSHINE, false);
                assertEquals("saved " + saved, Boolean.FALSE,
                        ReflectionHelpers.getStaticField(MoonBridge.class, "packed444Requested"));
                // And a session that uses it asks for it whatever the saved setting says.
                sessionPacked = true;
                stream(SUNSHINE, false);
                assertEquals("saved " + saved, Boolean.TRUE,
                        ReflectionHelpers.getStaticField(MoonBridge.class, "packed444Requested"));
            }
        } finally {
            sessionPacked = false;
        }
    }

    @Test
    public void packedHdrDropHasItsOwnNoticeAndDoesNotHideTheGenericHdrOne() throws Exception {
        int offered = MoonBridge.VIDEO_FORMAT_H264 | MoonBridge.VIDEO_FORMAT_H265 | MoonBridge.VIDEO_FORMAT_H265_MAIN10;
        String hdrUnavailable = RuntimeEnvironment.getApplication().getString(R.string.startup_hdr_unavailable);
        String eightBit = SUNSHINE.replace("<ServerCodecModeSupport>197377", "<ServerCodecModeSupport>257");
        String reason = "the GPU offers no 10-bit output";
        // The PC-side notice having been shown does not swallow the packed one...
        assertEquals(Collections.singletonList(hdrUnavailable), stream(eightBit, false, offered, false));
        List<String> packed = stream(SUNSHINE, false, offered, false, reason);
        assertEquals(1, packed.size());
        assertTrue(packed.get(0), packed.get(0).contains(reason));
        // ...each is said once, not on every stream...
        assertEquals(Collections.emptyList(), stream(SUNSHINE, false, offered, false, reason));
        assertEquals(Collections.emptyList(), stream(eightBit, false, offered, false));
        // ...and HDR working again re-arms both.
        assertEquals(Collections.emptyList(), stream(SUNSHINE, false, offered, false));
        assertEquals(1, stream(SUNSHINE, false, offered, false, reason).size());
        assertEquals(Collections.singletonList(hdrUnavailable), stream(eightBit, false, offered, false));
    }

    @Test
    public void aPcWithoutTenBitPackedModesIsSaidOnceAfterTheHandshake() throws Exception {
        Application app = RuntimeEnvironment.getApplication();
        String uuid = "packed-after-start-host";
        String reason = "host doesn't offer 10-bit full chroma";
        List<String> messages = new ArrayList<>();
        NvConnectionListener listener = (NvConnectionListener) Proxy.newProxyInstance(
                NvConnectionListener.class.getClassLoader(), new Class<?>[] {NvConnectionListener.class},
                (proxy, method, args) -> {
                    if (method.getName().equals("displayTransientMessage")) messages.add((String) args[0]);
                    return null;
                });
        // The handshake fell back to 8-bit packed: named once, not on every stream.
        NvConnection.noticePackedAfterStart(app, uuid, listener, true, null, reason);
        assertEquals(1, messages.size());
        assertTrue(messages.get(0), messages.get(0).contains(reason));
        NvConnection.noticePackedAfterStart(app, uuid, listener, true, null, reason);
        assertEquals(1, messages.size());
        // A stream that never asked for HDR says nothing; the launch's own reason is not said twice.
        NvConnection.noticePackedAfterStart(app, uuid, listener, false, null, null);
        NvConnection.noticePackedAfterStart(app, uuid, listener, true, "the display has no HDR10", "the display has no HDR10");
        assertEquals(1, messages.size());
        // A packed stream that kept 10-bit re-arms it.
        NvConnection.noticePackedAfterStart(app, uuid, listener, true, null, null);
        NvConnection.noticePackedAfterStart(app, uuid, listener, true, null, reason);
        assertEquals(2, messages.size());

        // The launch of a packed stream asking for HDR does not re-arm it: only the handshake knows.
        int offered = MoonBridge.VIDEO_FORMAT_H264 | MoonBridge.VIDEO_FORMAT_H265 | MoonBridge.VIDEO_FORMAT_H265_MAIN10;
        String launchReason = "the GPU offers no 10-bit output";
        try {
            sessionPacked = true;
            assertEquals(1, stream(SUNSHINE, false, offered, false, launchReason).size());
            assertEquals(Collections.emptyList(), stream(SUNSHINE, false, offered, false));
            assertEquals(Collections.emptyList(), stream(SUNSHINE, false, offered, false, launchReason));
        } finally {
            sessionPacked = false;
        }
    }

    @Test
    public void nativeFullChromaWinsOver10BitSdrOnlyWithAPcThatHasIt() throws Exception {
        int offered = MoonBridge.VIDEO_FORMAT_H264 | MoonBridge.VIDEO_FORMAT_H264_HIGH8_444 |
                MoonBridge.VIDEO_FORMAT_H265 | MoonBridge.VIDEO_FORMAT_H265_MAIN10;
        String preferred = RuntimeEnvironment.getApplication().getString(R.string.stream_sdr10_unavailable,
                RuntimeEnvironment.getApplication().getString(R.string.stream_sdr10_reason_yuv444));
        String with444 = SUNSHINE.replace("<ServerCodecModeSupport>197377", "<ServerCodecModeSupport>" + (197377 | 0x40000));
        // A PC without 4:4:4 keeps 10-bit SDR, whatever the phone could do.
        assertEquals(Collections.emptyList(), stream(SUNSHINE, false, offered, true));
        assertTrue(NvConnectionRecoveryTest.FakeHost.lastQuery, hdrMode(NvConnectionRecoveryTest.FakeHost.lastQuery, "0"));
        // A PC with both gets 4:4:4: said once, and the launch carries no hdrMode as 10-bit was not negotiated.
        assertEquals(Collections.singletonList(preferred), stream(with444, false, offered, true));
        assertFalse(NvConnectionRecoveryTest.FakeHost.lastQuery, NvConnectionRecoveryTest.FakeHost.lastQuery.contains("hdrMode"));
        assertFalse(NvConnectionRecoveryTest.FakeHost.lastHdr);
        assertEquals(0, negotiatedFormats() & MoonBridge.VIDEO_FORMAT_MASK_10BIT);
        assertNotEquals(0, negotiatedFormats() & MoonBridge.VIDEO_FORMAT_H264_HIGH8_444);
        assertEquals(Collections.emptyList(), stream(with444, false, offered, true));
        // HDR is not touched: the same offer for an HDR stream keeps its 10-bit formats.
        assertEquals(Collections.emptyList(), stream(with444, false, offered, false));
        assertTrue(NvConnectionRecoveryTest.FakeHost.lastHdr);
        // 10-bit working again re-arms the notice.
        assertEquals(Collections.emptyList(), stream(SUNSHINE, false, offered, true));
        assertEquals(Collections.singletonList(preferred), stream(with444, false, offered, true));
    }

    @Test
    public void fullChromaInOneCodecAnd10BitInAnotherStillGoesToFullChroma() throws Exception {
        // HEVC has 4:4:4 but no Main10 and AV1 has Main10: negotiating with the 10-bit bits in would drop
        // HEVC and its 4:4:4 format, so the choice is made without them.
        int offered = MoonBridge.VIDEO_FORMAT_H264 | MoonBridge.VIDEO_FORMAT_H265 | MoonBridge.VIDEO_FORMAT_H265_REXT8_444 |
                MoonBridge.VIDEO_FORMAT_AV1_MAIN8 | MoonBridge.VIDEO_FORMAT_AV1_MAIN10;
        String preferred = RuntimeEnvironment.getApplication().getString(R.string.stream_sdr10_unavailable,
                RuntimeEnvironment.getApplication().getString(R.string.stream_sdr10_reason_yuv444));
        String host = SUNSHINE.replace("<ServerCodecModeSupport>197377", "<ServerCodecModeSupport>" + (197377 | 0x80000));
        assertEquals(Collections.singletonList(preferred), stream(host, false, offered, true));
        assertEquals(0, negotiatedFormats() & MoonBridge.VIDEO_FORMAT_MASK_10BIT);
        assertNotEquals(0, negotiatedFormats() & MoonBridge.VIDEO_FORMAT_H265_REXT8_444);
        assertFalse(NvConnectionRecoveryTest.FakeHost.lastQuery, NvConnectionRecoveryTest.FakeHost.lastQuery.contains("hdrMode"));
        // Without the setting the 10-bit AV1 format is negotiated as before.
        stream(host, false, offered, false);
        assertNotEquals(0, negotiatedFormats() & MoonBridge.VIDEO_FORMAT_AV1_MAIN10);
    }

    private static boolean hdrMode(String query, String value) {
        return query.contains("&hdrMode=" + value + "&") || query.endsWith("&hdrMode=" + value);
    }

    @Test
    public void aPackedLaunchKnowsThePcsModesFromServerinfoElseItsLastDescribe() throws Exception {
        Application app = RuntimeEnvironment.getApplication();
        String uuid = "3c0e9b6e-6d8b-4a43-9c2e-5f1d1a2b3c4d";
        String withModes = SUNSHINE.replace("</root>", "<Packed444Modes>1</Packed444Modes></root>");
        int offered = MoonBridge.VIDEO_FORMAT_H264 | MoonBridge.VIDEO_FORMAT_H265 | MoonBridge.VIDEO_FORMAT_H265_MAIN10;
        try {
            com.limelight.computers.HostPackedModes.store(app, uuid, 0);
            sessionPacked = true;
            // Nothing known: the renderer keeps today's behaviour.
            stream(SUNSHINE, false, offered, false);
            assertEquals(Integer.valueOf(-1), hostPackedModesSeen);
            // The last DESCRIBE, when serverinfo does not say...
            com.limelight.computers.HostPackedModes.store(app, uuid, 0b1111);
            stream(SUNSHINE, false, offered, false);
            assertEquals(Integer.valueOf(0b1111), hostPackedModesSeen);
            // ...and serverinfo over it when it does.
            stream(withModes, false, offered, false);
            assertEquals(Integer.valueOf(0b0001), hostPackedModesSeen);
            // A session without packed chroma does not ask.
            sessionPacked = false;
            stream(withModes, false, offered, false);
            assertNull(hostPackedModesSeen);
        } finally {
            sessionPacked = false;
            com.limelight.computers.HostPackedModes.store(app, uuid, 0);
        }

        ComputerDetails details = new ComputerDetails();
        details.uuid = "packed-modes-host";
        assertEquals(-1, NvConnection.hostPackedModesBeforeLaunch(app, details));
        com.limelight.computers.HostPackedModes.store(app, details.uuid, 0b0011);
        assertEquals(0b0011, NvConnection.hostPackedModesBeforeLaunch(app, details));
        details.packed444Modes = 0;
        assertEquals(0, NvConnection.hostPackedModesBeforeLaunch(app, details));
    }

    @Test
    public void theStreamOnlyTrustsPyroWaveBitsFromHostsThatShowPyroWave() throws Exception {
        stream(SUNSHINE.replace("<ServerCodecModeSupport>197377", "<ServerCodecModeSupport>" + (197377 | 0x07800000)), false);
        assertEquals(197377, NvConnectionHapticsFlagTest.CapturingBridge.serverCodecModeSupport);
        assertEquals(0, NvConnectionHapticsFlagTest.CapturingBridge.hostFeatures);
        stream(TestHosts.serverInfo("vibepollo-serverinfo.xml"), false);
        assertEquals(126026497, NvConnectionHapticsFlagTest.CapturingBridge.serverCodecModeSupport);
        assertEquals(com.limelight.nvstream.http.HostCapabilities.NATIVE_PYROWAVE_RECORDS,
                NvConnectionHapticsFlagTest.CapturingBridge.hostFeatures);
    }
}

package com.limelight.nvstream;

import android.app.Application;
import android.content.Context;

import com.limelight.PcViewLifecycleTest;
import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.LimelightCryptoProvider;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.lang.reflect.Proxy;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.Assert.*;

/**
 * An automatic start was let through on what a poll saw (nothing running), or held at a PIN for as long as the
 * player took; the serverinfo read when the stream starts is the first to say another game runs now.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class NvConnectionNeverReplaceTest {
    private final Context context = RuntimeEnvironment.getApplication();
    private final AtomicInteger quits = new AtomicInteger();
    private final AtomicInteger stopped = new AtomicInteger();
    private final List<String> messages = new ArrayList<>();

    private NvHTTP pcRunning(int game) throws Exception {
        return new NvHTTP(new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "device", null,
                new LimelightCryptoProvider() {
                    public java.security.cert.X509Certificate getClientCertificate() { return null; }
                    public java.security.PrivateKey getClientPrivateKey() { return null; }
                    public byte[] getPemEncodedClientCertificate() { return new byte[0]; }
                    public String encodeBase64String(byte[] data) { return ""; }
                }) {
            @Override public int getCurrentGame(String serverInfo) { return game; }
            @Override public boolean quitApp() {
                quits.incrementAndGet();
                return true;
            }
        };
    }

    private ConnectionContext start(boolean neverReplace, NvApp app) {
        ConnectionContext start = new ConnectionContext();
        start.streamConfig = new StreamConfiguration.Builder().setApp(app).setNeverReplace(neverReplace).build();
        start.connListener = (NvConnectionListener) Proxy.newProxyInstance(
                NvConnectionListener.class.getClassLoader(), new Class<?>[] {NvConnectionListener.class},
                (proxy, method, args) -> {
                    if (method.getName().equals("displayMessage")) messages.add((String) args[0]);
                    if (method.getName().equals("automaticStartStopped")) stopped.incrementAndGet();
                    return null;
                });
        return start;
    }

    private static ComputerDetails freshDetails() {
        ComputerDetails details = new ComputerDetails();
        details.name = "Tower";
        details.runningGameId = 99;
        return details;
    }

    @Test
    public void anAutomaticStartStopsAtAFreshReportOfAnotherGameAndQuitsNothing() throws Exception {
        NvApp app = new NvApp("Portal", 11, false);
        NvConnection connection = new NvConnection(context, new ComputerDetails.AddressTuple("192.0.2.1", 47989),
                47984, "device", new StreamConfiguration.Builder().setApp(app).build(), null, null) {
            @Override protected boolean quitAndLaunch(NvHTTP h, ConnectionContext c) {
                fail("no tap, no quit");
                return true;
            }
        };
        assertFalse(connection.launchOrResume(pcRunning(99), start(true, app), freshDetails(), app, ""));
        assertEquals(0, quits.get());
        assertEquals(List.of(context.getString(R.string.open_play_other_device, "Tower")), messages);
        assertEquals("nothing failed: the next start is not skipped for it", 1, stopped.get());
    }

    @Test
    public void aStartThePlayerPressedStillQuitsAndLaunchesAsBefore() throws Exception {
        NvApp app = new NvApp("Portal", 11, false);
        AtomicInteger replaced = new AtomicInteger();
        NvConnection connection = new NvConnection(context, new ComputerDetails.AddressTuple("192.0.2.1", 47989),
                47984, "device", new StreamConfiguration.Builder().setApp(app).build(), null, null) {
            @Override protected boolean quitAndLaunch(NvHTTP h, ConnectionContext c) {
                replaced.incrementAndGet();
                return true;
            }
        };
        assertTrue(connection.launchOrResume(pcRunning(99), start(false, app), freshDetails(), app, ""));
        assertEquals(1, replaced.get());
        assertEquals(0, stopped.get());
    }
}

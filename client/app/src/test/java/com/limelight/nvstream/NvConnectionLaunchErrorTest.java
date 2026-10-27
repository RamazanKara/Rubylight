package com.limelight.nvstream;

import android.app.Application;
import android.content.Context;

import com.limelight.R;
import com.limelight.nvstream.av.video.VideoDecoderRenderer;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostHttpResponseException;
import com.limelight.nvstream.http.LaunchConfirmation;
import com.limelight.nvstream.http.LimelightCryptoProvider;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.lang.reflect.Proxy;
import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, shadows = NvConnectionRecoveryTest.FakeHost.class)
public class NvConnectionLaunchErrorTest {
    private static final String ENCODER_FAILURE =
            "Failed to initialize video capture/encoding. Is a display connected and turned on?";
    // The Sunshine reply that asks this device to confirm replacing the running app.
    private static final String REPLACE =
            "An app is already running. Launch this app again within 60 seconds to confirm that you want to close it.";

    private final Context context = RuntimeEnvironment.getApplication();

    @Before public void freeHost() {
        NvConnectionRecoveryTest.FakeHost.initial = "<root status_code=\"200\"><appversion>7.1.431.0</appversion>" +
                "<uniqueid>test-host</uniqueid><HttpsPort>47984</HttpsPort><state>SUNSHINE_SERVER_FREE</state>" +
                "<currentgame>0</currentgame><currentgameuuid></currentgameuuid><PairStatus>1</PairStatus></root>";
        NvConnectionRecoveryTest.FakeHost.resumeResult = false;
        NvConnectionRecoveryTest.FakeHost.requests.clear();
    }

    @Test public void theHostsOwnReasonIsShownWithItsCode() throws Exception {
        NvConnectionRecoveryTest.FakeHost.resumeFailure = new HostHttpResponseException(503, ENCODER_FAILURE);
        Result result = launch(Confirmation.UNEXPECTED);
        assertEquals(List.of(503), result.failures);
        assertEquals(List.of(context.getString(R.string.startup_host_message, ENCODER_FAILURE, 503)), result.messages);
    }

    @Test public void aHostThatGivesNoReasonGetsTheGenericText() throws Exception {
        for (String reason : new String[] {"", "  ", null}) {
            NvConnectionRecoveryTest.FakeHost.resumeFailure = new HostHttpResponseException(500, reason);
            Result result = launch(Confirmation.UNEXPECTED);
            assertEquals(List.of(context.getString(R.string.startup_host_error, 500)), result.messages);
        }
    }

    @Test public void permissionErrorsKeepTheirGuidanceAndTheHostsReason() throws Exception {
        NvConnectionRecoveryTest.FakeHost.resumeFailure = new HostHttpResponseException(403, "Device disabled");
        Result result = launch(Confirmation.UNEXPECTED);
        assertEquals(List.of(context.getString(R.string.startup_permission_denied) + "\n\nDevice disabled"),
                result.messages);
    }

    @Test public void cancellingTheConfirmationIsNotAHostFailure() throws Exception {
        NvConnectionRecoveryTest.FakeHost.resumeFailure = new HostHttpResponseException(410, REPLACE);
        Result result = launch(Confirmation.CANCEL);
        assertEquals(List.of("launch"), NvConnectionRecoveryTest.FakeHost.requests);
        assertEquals(List.of(context.getString(R.string.startup_launch_cancelled)), result.messages);
        assertFalse(result.messages.get(0).contains("499"));
    }

    @Test public void anInterruptedConfirmationIsACancelNotATransportFailure() throws Exception {
        NvConnectionRecoveryTest.FakeHost.resumeFailure = new HostHttpResponseException(410, REPLACE);
        Result result = launch(Confirmation.INTERRUPT);
        assertEquals(List.of("launch"), NvConnectionRecoveryTest.FakeHost.requests);
        assertEquals(List.of(context.getString(R.string.startup_launch_cancelled)), result.messages);
        // No port flags: a cancelled launch never qualifies for automatic recovery.
        assertEquals(List.of(0), result.portFlags);
        assertEquals(List.of(499), result.failures);
    }

    @Test public void aReplyThisDeviceCannotReadIsNotShownAsTheHostsReason() throws Exception {
        // The status attribute is not a number: the host reported neither a code nor a reason.
        NvConnectionRecoveryTest.FakeHost.initial = "<root status_code=\"OK\"><appversion>7.1.431.0</appversion>" +
                "<uniqueid>test-host</uniqueid><PairStatus>1</PairStatus></root>";
        Result result = launch(Confirmation.UNEXPECTED);
        assertEquals(List.of(context.getString(R.string.startup_connection_failed)), result.messages);
        assertEquals("The host reported no error code", List.of(0), result.failures);
        assertEquals("Retrying cannot fix the host's reply", List.of(0), result.portFlags);
    }

    @Test public void clientSideFailuresUseTheirOwnText() {
        String timedOut = NvConnection.launchFailureMessage(context, new NvConnection.LocalLaunchFailure(
                408, R.string.startup_confirmation_timed_out, "Confirmation timed out."));
        assertEquals(context.getString(R.string.startup_confirmation_timed_out), timedOut);
        assertFalse(timedOut.contains(context.getString(R.string.startup_host_error, 408)));
    }

    private enum Confirmation { UNEXPECTED, CANCEL, INTERRUPT }

    private Result launch(Confirmation confirmation) throws Exception {
        NvApp app = new NvApp("Game", 73, false);
        app.setAppUuid("game-a");
        NvConnection connection = new NvConnection(context,
                new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "device",
                new StreamConfiguration.Builder().setApp(app).setResolution(1280, 720).setRefreshRate(60)
                        .setSupportedVideoFormats(MoonBridge.VIDEO_FORMAT_H264).build(), crypto(), null);
        Result result = new Result();
        NvConnectionListener listener = (NvConnectionListener) Proxy.newProxyInstance(
                NvConnectionListener.class.getClassLoader(), new Class<?>[] {NvConnectionListener.class},
                (proxy, method, args) -> {
                    switch (method.getName()) {
                        case "displayMessage": result.messages.add((String) args[0]); break;
                        case "stageFailed":
                            result.portFlags.add((Integer) args[1]);
                            result.failures.add((Integer) args[2]);
                            break;
                        case "launchConfirmationRequired":
                            // The person chooses Cancel on the confirmation dialog.
                            if (confirmation == Confirmation.CANCEL) ((LaunchConfirmation) args[0]).cancel();
                            // The launch thread is interrupted while it waits for the person.
                            else if (confirmation == Confirmation.INTERRUPT) Thread.currentThread().interrupt();
                            else fail("Unexpected launch confirmation");
                            break;
                        default: break;
                    }
                    return null;
                });
        connection.start(null, new VideoDecoderRenderer() {
            @Override public int setup(int format, int width, int height, int rate) { throw new AssertionError(); }
            @Override public void start() {}
            @Override public void stop() {}
            @Override public void cleanup() {}
            @Override public int getCapabilities() { return 0; }
            @Override public void setHdrMode(boolean enabled, byte[] metadata) {}
            @Override public int submitDecodeUnit(byte[] data, int length, int type, int frame, int frameType,
                    char latency, long received, long queued, long receivedNs, long presentationUs) { return 0; }
        }, listener);
        Thread worker = ConnectionStartup.await(connection);
        assertFalse("Connection startup did not finish", worker.isAlive());
        return result;
    }

    private static LimelightCryptoProvider crypto() {
        return new LimelightCryptoProvider() {
            @Override public java.security.cert.X509Certificate getClientCertificate() { return null; }
            @Override public java.security.PrivateKey getClientPrivateKey() { return null; }
            @Override public byte[] getPemEncodedClientCertificate() { return new byte[0]; }
            @Override public String encodeBase64String(byte[] data) { return ""; }
        };
    }

    private static final class Result {
        final List<String> messages = new ArrayList<>();
        final List<Integer> failures = new ArrayList<>();
        final List<Integer> portFlags = new ArrayList<>();
    }
}

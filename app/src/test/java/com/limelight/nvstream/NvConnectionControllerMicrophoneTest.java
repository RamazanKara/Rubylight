package com.limelight.nvstream;

import android.app.Application;
import com.limelight.binding.audio.MicrophoneCapture;
import com.limelight.binding.audio.MicrophoneStatus;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.util.ReflectionHelpers;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = NvConnectionControllerMicrophoneTest.MicrophoneBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class NvConnectionControllerMicrophoneTest {
    @Implements(MoonBridge.class)
    public static class MicrophoneBridge {
        static int stateQueries;
        @Implementation protected static void __staticInitializer__() { }
        @Implementation protected static int getMicrophoneState() {
            stateQueries++;
            return MoonBridge.MIC_STATE_READY;
        }
    }

    @Test public void reconnectMuteSnapshotPreservesTheLocalChoiceWithoutNativeQueries() {
        Connection connection = new Connection(73);
        ReflectionHelpers.setField(connection, "nativeConnected", false);
        int queries = MicrophoneBridge.stateQueries;
        assertFalse(connection.isMicrophoneMutedForReconnect());
        connection.setMicrophoneMuted(true);
        assertTrue(connection.isMicrophoneMutedForReconnect());
        connection.setMicrophoneMuted(false);
        assertFalse(connection.isMicrophoneMutedForReconnect());
        assertEquals(queries, MicrophoneBridge.stateQueries);
        assertTrue(connection.captures.isEmpty());
    }

    @Test public void reconnectMuteSnapshotIncludesDeniedUsbCaptureAndOtherReadyOwners() {
        Connection connection = new Connection(73);
        ReflectionHelpers.setField(connection, "nativeConnected", false);
        ReflectionHelpers.setField(connection, "micEnabled", false);
        ReflectionHelpers.setField(connection, "micPermitted", false);
        int queries = MicrophoneBridge.stateQueries;
        Object muted = new Object(), ready = new Object();
        connection.controllerMicrophoneState(muted, true, false, true);
        assertFalse(connection.isMicrophoneMuted());
        assertTrue(connection.isMicrophoneMutedForReconnect());
        connection.controllerMicrophoneState(muted, false, false, true);
        connection.controllerMicrophoneState(ready, true, true, false);
        assertTrue(connection.isMicrophoneMutedForReconnect());
        connection.controllerMicrophoneState(muted, false, false, false);
        assertFalse(connection.isMicrophoneMutedForReconnect());
        connection.controllerMicrophoneState(muted, false, false, true);
        connection.releaseControllerMicrophone(muted);
        assertTrue(connection.isMicrophoneMutedForReconnect());
        connection.setMicrophoneMuted(false);
        assertFalse(connection.isMicrophoneMutedForReconnect());
        assertEquals(queries, MicrophoneBridge.stateQueries);
        assertTrue(connection.captures.isEmpty());
    }

    @Test public void negotiationAndUnavailableUsbCaptureKeepTheOrdinaryMicrophone() {
        Connection connection = new Connection(73);
        connection.startOrdinary();
        Object pad = new Object();
        connection.controllerMicrophoneState(pad, false, false, false);
        connection.controllerMicrophoneState(pad, true, false, false);
        assertEquals(1, connection.captures.size());
        assertTrue(connection.latest().running);
        connection.controllerMicrophoneState(pad, true, true, false);
        assertFalse(connection.latest().running);
        assertEquals(MicrophoneStatus.SENDING, connection.getMicrophoneStatus());
        connection.controllerMicrophoneState(pad, true, false, false);
        assertEquals(2, connection.captures.size());
        assertTrue(connection.latest().running);
        connection.releaseControllerMicrophone(pad);
        assertEquals(2, connection.captures.size());
    }

    @Test public void oneRetiringControllerCannotReplaceAnotherControllersMicrophone() {
        Connection connection = new Connection(73);
        connection.startOrdinary();
        Object first = new Object(), second = new Object();
        connection.controllerMicrophoneState(first, true, true, false);
        connection.controllerMicrophoneState(second, true, true, false);
        connection.releaseControllerMicrophone(first);
        connection.releaseControllerMicrophone(first);
        assertEquals(1, connection.captures.size());
        assertFalse(connection.latest().running);
        connection.releaseControllerMicrophone(second);
        assertEquals(2, connection.captures.size());
        assertTrue(connection.latest().running);
    }

    @Test public void hostMuteAndUserMuteDoNotStartPhoneCapture() {
        Connection connection = new Connection(73);
        connection.startOrdinary();
        Object pad = new Object();
        connection.controllerMicrophoneState(pad, true, true, false);
        connection.controllerMicrophoneState(pad, true, false, true);
        assertEquals(1, connection.captures.size());
        assertFalse(connection.latest().running);
        assertEquals(MicrophoneStatus.MUTED, connection.getMicrophoneStatus());
        connection.setMicrophoneMuted(true);
        connection.controllerMicrophoneState(pad, true, false, false);
        assertEquals(1, connection.captures.size());
        assertEquals(MicrophoneStatus.MUTED, connection.getMicrophoneStatus());
    }

    @Test public void captureDenialKeepsHostMuteUntilTheHostExplicitlyUnmutes() {
        Connection connection = new Connection(73);
        connection.startOrdinary();
        Object pad = new Object();
        connection.controllerMicrophoneState(pad, true, true, false);
        connection.controllerMicrophoneState(pad, true, false, true);
        connection.controllerMicrophoneState(pad, false, false, true);
        connection.controllerMicrophoneState(pad, false, false, true);
        assertEquals(1, connection.captures.size());
        assertFalse(connection.latest().running);
        assertEquals(MicrophoneStatus.MUTED, connection.getMicrophoneStatus());

        connection.controllerMicrophoneState(pad, false, false, false);
        assertEquals(2, connection.captures.size());
        assertTrue(connection.latest().running);
        assertEquals(MicrophoneStatus.SENDING, connection.getMicrophoneStatus());
    }

    @Test public void captureDenialAndDetachKeepHostMuteUntilStreamMenuUnmute() {
        Connection connection = new Connection(73);
        connection.startOrdinary();
        Object pad = new Object();
        connection.controllerMicrophoneState(pad, true, true, false);
        connection.controllerMicrophoneState(pad, true, false, true);
        connection.controllerMicrophoneState(pad, false, false, true);
        connection.releaseControllerMicrophone(pad);
        connection.releaseControllerMicrophone(pad);
        assertEquals(1, connection.captures.size());
        assertFalse(connection.latest().running);
        assertTrue(connection.isMicrophoneMuted());
        assertEquals(MicrophoneStatus.MUTED, connection.getMicrophoneStatus());

        connection.setMicrophoneMuted(false);
        assertEquals(2, connection.captures.size());
        assertTrue(connection.latest().running);
        assertEquals(MicrophoneStatus.SENDING, connection.getMicrophoneStatus());
    }

    @Test public void permissionLossRevokedInputAndTeardownCannotRestartPhoneCapture() {
        for (int reason = 0; reason < 3; reason++) {
            Connection connection = new Connection(73);
            connection.startOrdinary();
            Object pad = new Object();
            connection.controllerMicrophoneState(pad, true, true, false);
            if (reason == 0) connection.permitted = false; // Permission/AppOps/global microphone mute.
            else if (reason == 1) {
                ComputerDetails denied = new ComputerDetails(); denied.permission = 0;
                ReflectionHelpers.setField(connection, "hostDetails", denied);
            } else ReflectionHelpers.setField(connection, "stopRequested", true);
            connection.controllerMicrophoneState(pad, true, false, false);
            connection.releaseControllerMicrophone(pad);
            assertEquals(1, connection.captures.size());
            assertFalse(connection.latest().running);
        }
    }

    @Test public void remoteMonitorCannotAcquireAControllerMicrophone() {
        Connection connection = new Connection(2147483505);
        connection.controllerMicrophoneState(new Object(), true, true, false);
        assertTrue(connection.captures.isEmpty());
        assertFalse(ReflectionHelpers.callInstanceMethod(connection, "controllerMicrophoneOwnsRoute"));
        assertFalse(connection.controllerMicrophoneEnabled());
    }

    @Test public void remoteMonitorNeverOpensThePhoneMicrophoneWhateverTheSetting() {
        Connection connection = new Connection(2147483505);
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.enableMic = true;
        connection.readMicrophoneSettings(prefs);
        assertFalse(ReflectionHelpers.getField(connection, "micEnabled"));
        assertFalse(ReflectionHelpers.getField(connection, "micPermitted"));
        connection.startOrdinary();
        connection.setMicrophoneMuted(false);
        assertTrue(connection.captures.isEmpty());
        assertEquals(MicrophoneStatus.OFF, connection.getMicrophoneStatus());
    }

    @Test public void keyboardOnlyAccessKeepsTheOrdinaryMicrophoneAvailable() {
        Connection connection = new Connection(73);
        ComputerDetails keyboardOnly = new ComputerDetails(); keyboardOnly.permission = 1 << 12;
        ReflectionHelpers.setField(connection, "hostDetails", keyboardOnly);
        assertTrue(connection.canSendInput());
        assertFalse(connection.controllerMicrophonePermitted());
        assertFalse(connection.beginControllerMicrophone());
        connection.setMicrophoneMuted(false);
        assertEquals(1, connection.captures.size());
        assertTrue(connection.latest().running);
        connection.controllerMicrophoneState(new Object(), true, true, false);
        assertFalse(ReflectionHelpers.callInstanceMethod(connection, "controllerMicrophoneOwnsRoute"));
        assertEquals(1, connection.captures.size());
        assertTrue(connection.latest().running);
    }

    @Test public void losingControllerPermissionRestoresTheOrdinaryMicrophoneForKeyboardAccess() {
        Connection connection = new Connection(73);
        connection.startOrdinary();
        Object pad = new Object();
        connection.controllerMicrophoneState(pad, true, true, false);
        assertFalse(connection.latest().running);
        ComputerDetails keyboardOnly = new ComputerDetails(); keyboardOnly.permission = 1 << 12;
        ReflectionHelpers.setField(connection, "hostDetails", keyboardOnly);
        connection.controllerMicrophoneState(pad, true, true, false);
        assertFalse(connection.controllerMicrophoneEnabled());
        assertFalse(ReflectionHelpers.callInstanceMethod(connection, "controllerMicrophoneOwnsRoute"));
        assertEquals(2, connection.captures.size());
        assertTrue(connection.latest().running);
    }

    @Test public void unavailableUsbMicCanRecoverAfterUnmuteAndPermissionRestoration() {
        Connection connection = new Connection(73);
        connection.startOrdinary();
        Object pad = new Object();
        connection.controllerMicrophoneState(pad, true, true, false);
        connection.setMicrophoneMuted(true);
        connection.controllerMicrophoneState(pad, true, false, false);
        connection.setMicrophoneMuted(false);
        assertEquals(2, connection.captures.size());
        assertTrue(connection.latest().running);
        connection.controllerMicrophoneState(pad, true, true, false);
        connection.permitted = false;
        connection.controllerMicrophoneState(pad, true, false, false);
        assertEquals(2, connection.captures.size());
        connection.permitted = true;
        connection.controllerMicrophoneState(pad, true, false, false);
        assertEquals(3, connection.captures.size());
        assertTrue(connection.latest().running);
    }

    @Test public void retiringAHostMutedControllerRequiresExplicitUnmuteForFallback() {
        Connection connection = new Connection(73);
        connection.startOrdinary();
        Object pad = new Object();
        connection.controllerMicrophoneState(pad, true, true, false);
        connection.controllerMicrophoneState(pad, true, false, true);
        connection.releaseControllerMicrophone(pad);
        assertEquals(1, connection.captures.size());
        assertTrue(connection.isMicrophoneMuted());
        assertEquals(MicrophoneStatus.MUTED, connection.getMicrophoneStatus());
        connection.setMicrophoneMuted(false);
        assertEquals(2, connection.captures.size());
        assertTrue(connection.latest().running);
    }

    @Test public void aMicrophoneThatFailsMidStreamMutesSaysSoAndUnmuteOpensAFreshOne() {
        Connection connection = new Connection(73);
        int[] notices = {0};
        connection.setMicrophoneNoticeListener(() -> notices[0]++);
        connection.startOrdinary();
        assertEquals(MicrophoneStatus.SENDING, connection.getMicrophoneStatus());
        connection.latest().failOnItsOwn();
        assertTrue(connection.isMicrophoneMuted());
        assertEquals("The mute control stays offered", MicrophoneStatus.MUTED, connection.getMicrophoneStatus());
        assertEquals(1, notices[0]);
        assertTrue(connection.takeMicrophoneFailure());
        assertFalse("Said once", connection.takeMicrophoneFailure());
        assertNull("Not reported as missing effects", connection.takeMicrophoneNotice());

        connection.setMicrophoneMuted(false);
        assertEquals(2, connection.captures.size());
        assertTrue(connection.latest().running);
        assertEquals(MicrophoneStatus.SENDING, connection.getMicrophoneStatus());

        // A second failure after the retry is said again.
        connection.latest().failOnItsOwn();
        assertEquals(2, notices[0]);
        assertTrue(connection.takeMicrophoneFailure());
    }

    @Test public void aFailedPhoneMicrophoneLetsTheUsbFallbackRunAfterUnmute() {
        Connection connection = new Connection(73);
        connection.startOrdinary();
        connection.latest().failOnItsOwn();
        Object pad = new Object();
        connection.controllerMicrophoneState(pad, true, true, false);
        connection.releaseControllerMicrophone(pad);
        assertEquals("Still muted, nothing opens by itself", 1, connection.captures.size());
        connection.setMicrophoneMuted(false);
        assertEquals(2, connection.captures.size());
        assertTrue(connection.latest().running);
    }

    @Test public void aStoppedOrReplacedCaptureReportingLateChangesNothing() {
        Connection connection = new Connection(73);
        int[] notices = {0};
        connection.setMicrophoneNoticeListener(() -> notices[0]++);
        connection.startOrdinary();
        Capture first = connection.latest();
        Object pad = new Object();
        connection.controllerMicrophoneState(pad, true, true, false);
        connection.releaseControllerMicrophone(pad);
        assertEquals(2, connection.captures.size());
        first.failOnItsOwn();
        assertFalse(connection.isMicrophoneMuted());
        assertTrue(connection.latest().running);
        assertEquals(0, notices[0]);
        assertFalse(connection.takeMicrophoneFailure());
    }

    @Test public void usbWorkerRouteTransitionsDoNotWaitForTheNativeConnectionLock() throws Exception {
        Connection connection = new Connection(73);
        connection.startOrdinary();
        Object pad = new Object();
        ExecutorService worker = Executors.newSingleThreadExecutor();
        try {
            synchronized (MoonBridge.class) {
                worker.submit(() -> connection.controllerMicrophoneState(pad, true, true, false))
                        .get(1, TimeUnit.SECONDS);
                worker.submit(() -> connection.releaseControllerMicrophone(pad)).get(1, TimeUnit.SECONDS);
            }
            assertEquals(2, connection.captures.size());
            assertTrue(connection.latest().running);
        } finally { worker.shutdownNow(); }
    }

    private static class Connection extends NvConnection {
        boolean permitted = true;
        final List<Capture> captures = new ArrayList<>();
        Connection(int appId) {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().setApp(new NvApp("Test", appId, false)).build(), null, null);
            ReflectionHelpers.setField(this, "micEnabled", true);
            ReflectionHelpers.setField(this, "micPermitted", true);
            ReflectionHelpers.setField(this, "nativeConnected", true);
        }
        @Override boolean microphoneCapturePermitted() { return permitted && canSendInput(); }
        @Override MicrophoneCapture createMicrophoneCapture(boolean muted, MicrophoneCapture.FailureListener failed) {
            Capture capture = new Capture(muted, failed); captures.add(capture); return capture;
        }
        void startOrdinary() { ReflectionHelpers.callInstanceMethod(this, "startMicrophone"); }
        Capture latest() { return captures.get(captures.size() - 1); }
    }

    private static class Capture extends MicrophoneCapture {
        boolean running;
        final MicrophoneCapture.FailureListener failed;
        Capture(boolean muted, MicrophoneCapture.FailureListener failed) {
            super((pcm, length) -> 0, muted);
            this.failed = failed;
        }
        @Override public synchronized void start() { running = true; }
        @Override public void stop() { running = false; }
        @Override public boolean isCapturing() { return running; }
        // What the capture thread does when recording fails on its own: release, then report.
        void failOnItsOwn() { running = false; failed.failed(this); }
    }
}

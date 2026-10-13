package com.limelight.binding.input.driver;

import android.app.Application;
import android.hardware.usb.UsbDevice;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadow.api.Shadow;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.*;
import static org.robolectric.util.ReflectionHelpers.ClassParameter.from;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class DualSenseControllerMicrophoneTest {
    @Test
    public void closingIdleWorkerPublishesLatestHostMuteEvenWhenCaptureWasDenied() {
        for (boolean accepted : new boolean[] {false, true}) {
            Fixture fixture = new Fixture(accepted);
            fixture.hostMute(true);
            // The native callback updates the driver; the idle capture worker has not polled yet.
            assertFalse(fixture.connection.isMicrophoneMutedForReconnect());
            fixture.microphone.closed();
            assertTrue(fixture.connection.isMicrophoneMutedForReconnect());
            assertTrue(fixture.connection.isMicrophoneMuted());
            assertNull(ReflectionHelpers.getField(fixture.controller, "microphoneOwner"));
            fixture.connection.setMicrophoneMuted(false);
            assertFalse(fixture.connection.isMicrophoneMutedForReconnect());
        }
    }

    @Test
    public void closingAfterPermissionRevocationStillPreservesTheLastHostMute() {
        Fixture fixture = new Fixture(true);
        fixture.hostMute(true);
        fixture.connection.controllerInputAllowed = false;
        fixture.microphone.closed();
        assertFalse(fixture.connection.canSendControllerInput());
        assertTrue(fixture.connection.isMicrophoneMuted());
        assertTrue(fixture.connection.isMicrophoneMutedForReconnect());
    }

    @Test
    public void closingIdleWorkerHonorsTheLatestExplicitHostUnmute() {
        Fixture fixture = new Fixture(true);
        fixture.hostMute(true);
        fixture.microphone.ready(false);
        assertTrue(fixture.connection.isMicrophoneMutedForReconnect());
        fixture.hostMute(false);
        fixture.microphone.closed();
        assertFalse(fixture.connection.isMicrophoneMutedForReconnect());
        assertFalse(fixture.connection.isMicrophoneMuted());
    }

    @Test
    public void staleClosedCallbackDoesNotPublishOrClearTheNewOwnersHostMute() {
        Fixture fixture = new Fixture(true);
        Object replacementOwner = new Object();
        ReflectionHelpers.setField(fixture.controller, "microphoneOwner", replacementOwner);
        DualSenseHaptics.Microphone replacement = fixture.sink(replacementOwner);
        fixture.hostMute(true);
        fixture.microphone.closed();
        assertSame(replacementOwner, ReflectionHelpers.getField(fixture.controller, "microphoneOwner"));
        assertFalse(fixture.connection.isMicrophoneMutedForReconnect());
        replacement.closed();
        assertTrue(fixture.connection.isMicrophoneMutedForReconnect());
    }

    private static final class Fixture {
        final Connection connection = new Connection();
        // Construct only the report handler. No USB connection, driver, audio worker, or native stream is started.
        final DualSenseController controller = new DualSenseController(
                Shadow.newInstanceOf(UsbDevice.class), null, 42, null, null);
        final DualSenseHaptics.Microphone microphone;

        Fixture(boolean accepted) {
            Object owner = new Object();
            ReflectionHelpers.setField(controller, "audioConnection", connection);
            ReflectionHelpers.setField(controller, "microphoneOwner", owner);
            ReflectionHelpers.setField(controller, "microphoneAccepted", accepted);
            microphone = sink(owner);
            microphone.ready(false);
        }

        DualSenseHaptics.Microphone sink(Object owner) {
            return ReflectionHelpers.callInstanceMethod(controller, "createMicrophoneSink",
                    from(NvConnection.class, connection), from(Object.class, owner));
        }

        void hostMute(boolean muted) {
            controller.controllerAudio(DualSenseAudioPacket.CONTROL,
                    new byte[] {1, 2, 0, 0, 0, 2, 0, 0, 0, 0, 0, (byte)(muted ? 0x10 : 0)});
        }
    }

    private static final class Connection extends NvConnection {
        boolean controllerInputAllowed = true;

        Connection() {
            super(null, new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }

        @Override public boolean canSendControllerInput() {
            return controllerInputAllowed && super.canSendControllerInput();
        }
    }
}

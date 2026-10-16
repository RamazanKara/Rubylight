package com.limelight.binding.input.driver;

import android.app.Application;
import android.hardware.usb.UsbDeviceConnection;

import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadow.api.Shadow;
import org.robolectric.util.ReflectionHelpers;

import java.util.concurrent.TimeUnit;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = {UsbPads.Connection.class, com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class DualSenseControllerStopTest {
    private final UsbPads.Recorder listener = new UsbPads.Recorder();

    @Before
    public void resetPads() {
        UsbPads.Connection.reset();
    }

    private DualSenseController controller() {
        return new DualSenseController(UsbPads.dualSense("/dev/bus/usb/001/005"),
                Shadow.newInstanceOf(UsbDeviceConnection.class), 42, listener, null);
    }

    @Test
    public void stopReturnsAtOnceAndReleasesThePadOnlyAfterItsUsbRead() throws Exception {
        UsbPads.Connection.inputReport = UsbPads.dualSenseReport();
        DualSenseController controller = controller();
        // The USB service's worker would run this.
        assertTrue(controller.start());
        UsbPads.await(() -> listener.has("added") && listener.has("state"));
        UsbPads.awaitSlowRead(400);

        long stop = System.nanoTime();
        controller.stop();
        long stopMs = TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - stop);
        // The main thread calls stop() on detach and at the end of a stream.
        assertTrue("stop() held its caller for " + stopMs + " ms", stopMs < 100);
        UsbPads.await(() -> UsbPads.Connection.closes.get() == 1 && listener.has("removed"));
        assertFalse("The connection was closed under a USB read", UsbPads.Connection.closedDuringRead);
        assertEquals(1, UsbPads.Connection.releases.get());
    }

    @Test
    public void stopKeepsTheHostsMicrophoneMuteForAReconnect() {
        Connection connection = new Connection();
        DualSenseController controller = controller();
        ReflectionHelpers.setField(controller, "audioConnection", connection);
        ReflectionHelpers.setField(controller, "microphoneOwner", new Object());
        // The host mutes the controller's microphone.
        controller.controllerAudio(DualSenseAudioPacket.CONTROL, new byte[] {1, 2, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0x10});
        assertFalse(connection.isMicrophoneMutedForReconnect());
        controller.stop();
        // The stream reads this right after stopping its controllers, before their USB release ends.
        assertTrue(connection.isMicrophoneMutedForReconnect());
    }

    private static final class Connection extends NvConnection {
        Connection() {
            super(null, new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }
    }
}

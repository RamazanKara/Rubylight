package com.limelight.binding.input.driver;

import android.app.Application;
import android.hardware.usb.UsbDevice;
import android.hardware.usb.UsbDeviceConnection;
import android.hardware.usb.UsbInterface;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadow.api.Shadow;
import org.robolectric.shadows.ShadowSystemClock;
import org.robolectric.util.ReflectionHelpers;

import java.time.Duration;
import java.util.Arrays;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 33, application = Application.class, shadows = UsbPads.Connection.class)
public class SwitchProOutputTest {
    private final UsbPads.Recorder listener = new UsbPads.Recorder();

    @Before
    public void resetPads() {
        UsbPads.Connection.reset();
    }

    // A pad past its USB start: endpoints found, nothing written yet.
    private SwitchProController startedPad() {
        UsbDevice device = UsbPads.switchPro("/dev/bus/usb/001/010");
        SwitchProController pad = new SwitchProController(device, Shadow.newInstanceOf(UsbDeviceConnection.class), 7, listener);
        UsbInterface hid = device.getInterface(0);
        ReflectionHelpers.setField(pad, "hidInterface", hid);
        ReflectionHelpers.setField(pad, "inEndpoint", hid.getEndpoint(0));
        ReflectionHelpers.setField(pad, "outEndpoint", hid.getEndpoint(1));
        return pad;
    }

    private static byte[] rumbleBytes(short low, short high) {
        byte[] report = new SwitchProReport.Output().rumble(low, high);
        return Arrays.copyOfRange(report, 2, 10);
    }

    @Test
    public void rumbleNeverWaitsForUsbAndOnlyTheNewestLevelIsSent() throws Exception {
        SwitchProController pad = startedPad();
        // The first level goes out and stays in USB until the gate opens.
        UsbPads.holdOutputs();
        pad.startOutput();
        pad.rumble((short) 1000, (short) 0);
        UsbPads.awaitOutputHeld();
        for (int i = 2; i <= 20; i++) {
            pad.rumble((short) (i * 1000), (short) 0);
        }
        assertTrue("rumble() waited for the USB write", UsbPads.Connection.outputHeld.get());
        UsbPads.releaseOutputs();

        UsbPads.await(() -> {
            int count = UsbPads.Connection.outputs.size();
            return count > 0 && Arrays.equals(rumbleBytes((short) 20000, (short) 0),
                    Arrays.copyOfRange(UsbPads.Connection.outputs.get(count - 1), 2, 10));
        });
        // The first level written while the rest queued up, then the newest: nothing in between.
        assertTrue("Wrote " + UsbPads.Connection.outputs.size() + " reports", UsbPads.Connection.outputs.size() <= 3);
        for (String thread : UsbPads.Connection.outputThreads) assertEquals("Switch Pro USB output", thread);

        pad.stop();
        UsbPads.await(() -> UsbPads.Connection.closes.get() == 1);
    }

    @Test
    public void inputReportsAreForwardedWithoutWritingOnTheInputThread() throws Exception {
        byte[] report = new byte[64];
        report[0] = 0x30;
        UsbPads.Connection.inputReport = report;
        UsbPads.Connection.outputWriteMs = 30;
        SwitchProController pad = startedPad();
        pad.startOutput();
        Thread input = new Thread(() -> ReflectionHelpers.callInstanceMethod(pad, "readInput"), "Switch Pro USB input");
        ReflectionHelpers.setField(pad, "inputThread", input);
        input.start();
        UsbPads.await(() -> listener.has("added"));
        pad.rumble((short) 30000, (short) 30000);
        pad.setPlayerNumber(1);
        UsbPads.await(() -> UsbPads.Connection.outputs.size() >= 2 && listener.count("state") > 20);
        // An active rumble is repeated while input keeps arriving.
        ShadowSystemClock.advanceBy(Duration.ofMillis(100));
        UsbPads.await(() -> UsbPads.Connection.outputs.size() >= 3);
        assertFalse("A USB write ran on the input thread", UsbPads.Connection.outputThreads.contains("Switch Pro USB input"));

        pad.stop();
        UsbPads.await(() -> UsbPads.Connection.closes.get() == 1 && listener.has("removed"));
        assertFalse(UsbPads.Connection.closedDuringRead);
    }
}

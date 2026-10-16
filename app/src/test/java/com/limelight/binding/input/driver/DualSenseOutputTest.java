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
import org.robolectric.util.ReflectionHelpers;

import java.util.concurrent.TimeUnit;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 33, application = Application.class, shadows = {UsbPads.Connection.class,
        com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class DualSenseOutputTest {
    private final UsbPads.Recorder listener = new UsbPads.Recorder();

    @Before
    public void resetPads() {
        UsbPads.Connection.reset();
    }

    // A pad past its USB start: endpoints found, nothing written yet, output thread not running.
    private DualSenseController startedPad() {
        UsbDevice device = UsbPads.dualSense("/dev/bus/usb/001/005");
        DualSenseController pad = new DualSenseController(device, Shadow.newInstanceOf(UsbDeviceConnection.class), 42, listener, null);
        UsbInterface hid = device.getInterface(0);
        ReflectionHelpers.setField(pad, "hidInterface", hid);
        ReflectionHelpers.setField(pad, "inEndpoint", hid.getEndpoint(0));
        ReflectionHelpers.setField(pad, "outEndpoint", hid.getEndpoint(1));
        ReflectionHelpers.setField(pad, "output", new DualSenseReport.Output(true));
        return pad;
    }

    @Test
    public void outputNeverWaitsForUsbAndOnlyTheNewestRumbleIsSent() throws Exception {
        UsbPads.Connection.outputWriteMs = 100;
        DualSenseController pad = startedPad();
        pad.startOutput();
        long start = System.nanoTime();
        for (int i = 1; i <= 20; i++) {
            pad.rumble((short) (i * 1000), (short) 0);
        }
        pad.setLed((byte) 1, (byte) 2, (byte) 3);
        pad.setPlayerNumber(1);
        pad.setAdaptiveTriggers((byte) DualSenseReport.LEFT_TRIGGER, (byte) 2, (byte) 0, new byte[10], new byte[10]);
        long elapsedMs = TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - start);
        assertTrue("Output held its caller for " + elapsedMs + " ms", elapsedMs < 50);

        // The newest level is 20000 >> 8, in the low-frequency byte.
        UsbPads.await(() -> UsbPads.Connection.outputs.stream().anyMatch(report -> report[4] == (byte) (20000 >> 8)) &&
                UsbPads.Connection.outputs.size() >= 4);
        // The first level written while the rest queued up, then the newest: nothing in between.
        assertTrue("Wrote " + UsbPads.Connection.outputs.size() + " reports", UsbPads.Connection.outputs.size() <= 6);
        for (String thread : UsbPads.Connection.outputThreads) assertEquals("DualSense USB output", thread);

        pad.stop();
        UsbPads.await(() -> UsbPads.Connection.closes.get() == 1);
    }

    @Test
    public void effectsForBothTriggersQueuedTogetherGoOutInOneReport() throws Exception {
        DualSenseController pad = startedPad();
        pad.setAdaptiveTriggers((byte) DualSenseReport.LEFT_TRIGGER, (byte) 2, (byte) 0, new byte[10], null);
        pad.setAdaptiveTriggers((byte) DualSenseReport.RIGHT_TRIGGER, (byte) 0, (byte) 3, null, new byte[10]);
        // An effect of the wrong size is dropped as before.
        pad.setAdaptiveTriggers((byte) DualSenseReport.RIGHT_TRIGGER, (byte) 0, (byte) 4, null, new byte[3]);
        pad.startOutput();
        UsbPads.await(() -> !UsbPads.Connection.outputs.isEmpty());
        byte[] report = UsbPads.Connection.outputs.get(0);
        assertEquals(DualSenseReport.LEFT_TRIGGER | DualSenseReport.RIGHT_TRIGGER,
                report[1] & (DualSenseReport.LEFT_TRIGGER | DualSenseReport.RIGHT_TRIGGER));
        assertEquals(2, report[22]);
        assertEquals(3, report[11]);
        pad.stop();
        UsbPads.await(() -> UsbPads.Connection.closes.get() == 1);
    }

    @Test
    public void inputReportsAreForwardedWithoutWritingOnTheInputThread() throws Exception {
        UsbPads.Connection.inputReport = UsbPads.dualSenseReport();
        UsbPads.Connection.outputWriteMs = 30;
        DualSenseController pad = startedPad();
        pad.startOutput();
        Thread input = new Thread(() -> ReflectionHelpers.callInstanceMethod(pad, "readInput"), "DualSense USB input");
        ReflectionHelpers.setField(pad, "inputThread", input);
        input.start();
        UsbPads.await(() -> listener.has("added"));
        pad.rumble((short) 30000, (short) 30000);
        pad.setLed((byte) 9, (byte) 9, (byte) 9);
        UsbPads.await(() -> UsbPads.Connection.outputs.size() >= 2 && listener.count("state") > 20);
        assertFalse("A USB write ran on the input thread", UsbPads.Connection.outputThreads.contains("DualSense USB input"));

        pad.stop();
        UsbPads.await(() -> UsbPads.Connection.closes.get() == 1 && listener.has("removed"));
        assertFalse(UsbPads.Connection.closedDuringRead);
    }
}

package com.limelight.binding.input.driver;

import android.app.Application;
import android.hardware.usb.UsbDevice;
import android.hardware.usb.UsbDeviceConnection;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadow.api.Shadow;
import org.robolectric.util.ReflectionHelpers;

import java.lang.reflect.Proxy;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class DualSenseControllerStopTest {
    @Test
    public void stopWaitsForTheUsbWorkerBeforeClosingTheConnection() throws Exception {
        UsbDriverListener listener = (UsbDriverListener) Proxy.newProxyInstance(
                UsbDriverListener.class.getClassLoader(), new Class<?>[] {UsbDriverListener.class},
                (proxy, method, args) -> null);
        DualSenseController controller = new DualSenseController(Shadow.newInstanceOf(UsbDevice.class),
                Shadow.newInstanceOf(UsbDeviceConnection.class), 42, listener, null);
        CountDownLatch reading = new CountDownLatch(1);
        AtomicBoolean workerRunning = new AtomicBoolean(true);
        Thread worker = new Thread(() -> {
            reading.countDown();
            // Like a USB read, this ignores interrupts and ends at its own timeout.
            long end = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(200);
            for (long left; (left = end - System.nanoTime()) > 0; ) {
                try {
                    TimeUnit.NANOSECONDS.sleep(left);
                } catch (InterruptedException ignored) {
                }
            }
            workerRunning.set(false);
        }, "DualSense USB input");
        ReflectionHelpers.setField(controller, "inputThread", worker);
        worker.start();
        assertTrue(reading.await(1, TimeUnit.SECONDS));

        controller.stop();

        assertFalse(workerRunning.get());
        worker.join(1000);
    }
}

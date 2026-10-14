package com.limelight.binding.input.driver;

import android.app.Application;
import android.hardware.usb.UsbDevice;
import android.hardware.usb.UsbDeviceConnection;
import android.hardware.usb.UsbManager;

import com.limelight.nvstream.jni.MoonBridge;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.util.ReflectionHelpers;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;

import static org.junit.Assert.*;
import static org.robolectric.util.ReflectionHelpers.ClassParameter.from;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, shadows = DualSenseHapticsTest.FailingWaveformBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class DualSenseHapticsTest {
    @Implements(value = MoonBridge.class, isInAndroidSdk = false)
    public static class FailingWaveformBridge {
        static volatile CountDownLatch writeFails;

        @Implementation protected static void __staticInitializer__() { }

        @Implementation public static boolean writeUsbHaptics(long handle, byte[] samples) {
            awaitUninterruptibly(writeFails);
            return false;
        }
    }

    @Test
    public void closingDoesNotWaitForAWorkerStuckInUsbAudioCleanup() throws Exception {
        FailingWaveformBridge.writeFails = new CountDownLatch(1);
        CountDownLatch microphoneStuck = new CountDownLatch(1);
        CountDownLatch unstick = new CountDownLatch(1);
        UsbDeviceConnection connection = ReflectionHelpers.callConstructor(UsbDeviceConnection.class,
                from(UsbDevice.class, null));
        DualSenseUsbAudio audio = ReflectionHelpers.callConstructor(DualSenseUsbAudio.class,
                from(UsbManager.class, null), from(UsbDevice.class, null), from(UsbDeviceConnection.class, connection));
        DualSenseHaptics haptics = new DualSenseHaptics(audio, active -> { });
        haptics.microphone(new DualSenseHaptics.Microphone() {
            @Override public boolean enabled() { return false; }
            @Override public boolean start() { return false; }
            @Override public void finish() { }
            @Override public void samples(byte[] pcm, int length) { }
            @Override public void ready(boolean ready) {
                // A capture-side callback that does not return while the controller fails.
                microphoneStuck.countDown();
                awaitUninterruptibly(unstick);
            }
            @Override public void closed() { }
        });
        ExecutorService mainThread = Executors.newSingleThreadExecutor();
        try {
            assertTrue(microphoneStuck.await(2, TimeUnit.SECONDS));
            // The waveform output fails; its worker now waits for the stuck microphone worker.
            FailingWaveformBridge.writeFails.countDown();
            long started = System.nanoTime();
            mainThread.submit(haptics::close).get(2, TimeUnit.SECONDS);
            long waitedMs = TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - started);
            assertTrue("Stopping the controller waited " + waitedMs + " ms",
                    waitedMs < DualSenseHaptics.CLOSE_WAIT_MS + 1000);
            assertFalse(haptics.isReady());
        } finally {
            unstick.countDown();
            mainThread.shutdownNow();
        }
    }

    private static void awaitUninterruptibly(CountDownLatch latch) {
        boolean interrupted = false;
        while (true) {
            try {
                latch.await();
                break;
            } catch (InterruptedException e) {
                interrupted = true;
            }
        }
        if (interrupted) Thread.currentThread().interrupt();
    }
}

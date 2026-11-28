package com.limelight.binding.input.driver;

import android.app.Application;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.hardware.usb.UsbDevice;
import android.hardware.usb.UsbDeviceConnection;
import android.hardware.usb.UsbManager;
import android.os.Looper;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ServiceController;
import org.robolectric.annotation.Config;
import org.robolectric.shadow.api.Shadow;
import org.robolectric.shadows.ShadowUsbManager;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CountDownLatch;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

// Android 13 registers the service's unexported USB receiver without a manifest permission.
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 33, application = Application.class,
        shadows = {UsbPads.Connection.class, com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class UsbControllerLifecycleTest {
    private final Application app = RuntimeEnvironment.getApplication();
    private final UsbPads.Recorder listener = new UsbPads.Recorder();
    private ServiceController<UsbDriverService> service;

    @Before
    public void resetPads() {
        UsbPads.Connection.reset();
    }

    @After
    public void stopService() {
        if (service != null) service.destroy();
    }

    private ShadowUsbManager usbManager() {
        return shadowOf((UsbManager) app.getSystemService(Context.USB_SERVICE));
    }

    private UsbDriverService.UsbDriverBinder bind(UsbDevice pad) {
        android.preference.PreferenceManager.getDefaultSharedPreferences(app).edit()
                .putBoolean("checkbox_usb_driver", true).putBoolean("checkbox_usb_dualsense", true).commit();
        usbManager().addOrUpdateUsbDevice(pad, true);
        service = Robolectric.buildService(UsbDriverService.class).create();
        UsbDriverService.UsbDriverBinder binder = (UsbDriverService.UsbDriverBinder) service.get().onBind(new Intent());
        binder.setListener(listener);
        return binder;
    }

    private void detach(UsbDevice pad) {
        usbManager().removeUsbDevice(pad);
        app.sendBroadcast(new Intent(UsbManager.ACTION_USB_DEVICE_DETACHED).putExtra(UsbManager.EXTRA_DEVICE, pad));
    }

    @Test
    public void aPadThatStallsWhileStartingNeitherHoldsTheMainThreadNorLeaks() throws Exception {
        // A failing pad: its feature reads stall until the gate opens, so nothing that waited
        // for one can come back before then, however slowly the test machine runs.
        UsbPads.Connection.controlGate = new CountDownLatch(1);
        UsbPads.Connection.holdControls = true;
        UsbDevice pad = UsbPads.dualSense("/dev/bus/usb/001/002");
        UsbDriverService.UsbDriverBinder binder = bind(pad);
        binder.start();
        assertEquals("Starting the USB driver waited for the pad", 0, UsbPads.Connection.controlsEnded.get());
        UsbPads.await(UsbPads.Connection.controlHeld::get);
        assertFalse(UsbPads.Connection.controlThreads.contains(Thread.currentThread().getName()));

        // The pad is unplugged while it is still starting.
        detach(pad);
        shadowOf(Looper.getMainLooper()).idle();
        assertTrue("Unplugging waited for the stalled pad", UsbPads.Connection.controlHeld.get());
        assertEquals(0, UsbPads.Connection.controlsEnded.get());

        UsbPads.Connection.controlGate.countDown();
        UsbPads.await(() -> UsbPads.Connection.closes.get() == 1);
        Thread.sleep(100);
        UsbPads.await(() -> true);
        assertEquals("The connection is closed once", 1, UsbPads.Connection.closes.get());
        assertEquals("The claimed interface is released", UsbPads.Connection.claims.get(), UsbPads.Connection.releases.get());
        assertFalse("A pad stopped while starting is never announced", listener.has("added"));
    }

    @Test
    public void aSwitchProThatNeverAnswersItsHandshakeIsReleasedAndReported() throws Exception {
        android.preference.PreferenceManager.getDefaultSharedPreferences(app).edit()
                .putBoolean("checkbox_usb_switch_pro", true).commit();
        // Reads time out: the pad never acknowledges the USB handshake.
        UsbDevice pad = UsbPads.switchPro("/dev/bus/usb/001/009");
        bind(pad).start();
        UsbPads.await(() -> listener.has("start failed") && UsbPads.Connection.closes.get() == 1);
        assertEquals("The claimed interface is released", UsbPads.Connection.claims.get(), UsbPads.Connection.releases.get());
        assertFalse("A pad that never started is never announced", listener.has("added"));
    }

    @Test
    public void noInputReachesTheListenerOnceThePadIsDetached() throws Exception {
        UsbPads.Connection.inputReport = UsbPads.dualSenseReport();
        UsbDevice pad = UsbPads.dualSense("/dev/bus/usb/001/003");
        bind(pad).start();
        // The pad announces itself once Android's own driver has let go (after a second), then reports.
        UsbPads.await(() -> listener.has("added") && listener.count("state") > 3);

        // Marks where the main thread handles the detach in the stream of delivered events.
        app.registerReceiver(new BroadcastReceiver() {
            @Override public void onReceive(Context context, Intent intent) {
                listener.events.add("detached");
            }
        }, new IntentFilter(UsbManager.ACTION_USB_DEVICE_DETACHED));
        detach(pad);
        // The pad keeps reporting while the detach waits for its turn on the main thread.
        Thread.sleep(40);
        UsbPads.await(() -> UsbPads.Connection.closes.get() == 1 && listener.has("removed"));

        List<String> afterDetach;
        synchronized (listener.events) {
            afterDetach = new ArrayList<>(listener.events.subList(listener.events.indexOf("detached"), listener.events.size()));
        }
        assertFalse("Input after the detach: " + afterDetach, afterDetach.contains("state"));
        assertTrue(afterDetach.contains("removed"));
        assertFalse(UsbPads.Connection.closedDuringRead);
    }

    @Test
    public void anXboxPadIsReleasedOnlyAfterItsUsbReadAndNotOnTheCallersThread() throws Exception {
        // An Xbox pad sends reports only when something changes, so its reads often wait.
        UsbPads.Connection.inputReport = new byte[64];
        XboxOneController controller = new XboxOneController(UsbPads.xboxOne("/dev/bus/usb/001/004"),
                Shadow.newInstanceOf(UsbDeviceConnection.class), 7, listener);
        // The USB service's worker would run this.
        assertTrue(controller.start());
        UsbPads.await(() -> listener.has("added"));
        UsbPads.holdReads();

        controller.stop();
        boolean readStillHeld = UsbPads.Connection.readHeld.get();
        UsbPads.releaseReads();
        assertTrue("stop() waited for the pad's USB read", readStillHeld);
        UsbPads.await(() -> UsbPads.Connection.closes.get() == 1 && listener.has("removed"));
        assertFalse("The connection was closed under a USB read", UsbPads.Connection.closedDuringRead);
        assertEquals(UsbPads.Connection.claims.get(), UsbPads.Connection.releases.get());
    }

    @Test
    public void autoForwardedControllerIsSkippedOnlyWhileForwardingIsActive() throws Exception {
        UsbPads.Connection.inputReport = UsbPads.dualSenseReport();
        UsbDevice pad = UsbPads.dualSense("/dev/bus/usb/001/011");
        app.getSharedPreferences("usb_forwarding", Context.MODE_PRIVATE).edit()
                .putStringSet("auto", java.util.Collections.singleton(
                        String.format(java.util.Locale.ROOT, "%04x:%04x", pad.getVendorId(), pad.getProductId())))
                .commit();
        com.limelight.binding.usb.UsbForwarding forwarding = null;
        try {
            com.limelight.preferences.PreferenceConfiguration config = new com.limelight.preferences.PreferenceConfiguration();
            config.usbDualSense = true;
            // Stored "auto" list alone outside an active stream forwarding that device must not block claiming.
            assertTrue(UsbDriverService.shouldClaimDevice(app, pad, config));
            bind(pad).start();
            UsbPads.await(() -> listener.has("added"));
            assertEquals("Inactive auto pref must not prevent UsbDriverService claiming", 1, UsbPads.Connection.claims.get());

            // Hold the active USB read while asserting forwarding ownership so UsbForwarding's
            // worker waits in awaitDeviceRelease instead of racing USB_NO_STREAM onto main.
            forwarding = new com.limelight.binding.usb.UsbForwarding(app, config, null, ignored -> {});
            forwarding.setDeviceReleaser(UsbDriverService::releaseActiveDevice);
            UsbPads.Connection.releases.set(0);
            shadowOf((UsbManager) app.getSystemService(Context.USB_SERVICE)).addOrUpdateUsbDevice(pad, true);
            UsbPads.holdReads();
            forwarding.choose(pad);
            shadowOf(Looper.getMainLooper()).idle();
            assertFalse(UsbDriverService.shouldClaimDevice(app, pad, config));
            assertTrue(forwarding.isForwarded(pad));
        } finally {
            UsbPads.releaseReads();
            if (service != null) {
                service.destroy();
                service = null;
            }
            if (forwarding != null) {
                forwarding.stop();
                forwarding.awaitDetaches(2000);
                shadowOf(Looper.getMainLooper()).idle();
            }
            UsbDriverService.awaitDeviceRelease(pad, 2000);
            shadowOf(Looper.getMainLooper()).idle();
            app.getSharedPreferences("usb_forwarding", Context.MODE_PRIVATE).edit().clear().commit();
        }
    }

    @Test
    public void releasingActiveDeviceStopsControllerAndFailedForwardAllowsReclaimWithoutReplug() throws Exception {
        UsbPads.Connection.inputReport = UsbPads.dualSenseReport();
        UsbDevice pad = UsbPads.dualSense("/dev/bus/usb/001/012");
        bind(pad).start();
        UsbPads.await(() -> listener.has("added"));

        UsbDriverService.releaseActiveDevice(pad);
        UsbDriverService.awaitDeviceRelease(pad, 2000);
        UsbPads.await(() -> UsbPads.Connection.closes.get() == 1 && listener.has("removed"));
        assertEquals(UsbPads.Connection.claims.get(), UsbPads.Connection.releases.get());

        // While forwarding still owns the device, a stray attach broadcast must not reclaim it.
        app.sendBroadcast(new Intent(UsbManager.ACTION_USB_DEVICE_ATTACHED).putExtra(UsbManager.EXTRA_DEVICE, pad));
        shadowOf(Looper.getMainLooper()).idleFor(1500, java.util.concurrent.TimeUnit.MILLISECONDS);
        assertEquals("Released device must not be reclaimed while forwarding holds it", 1, UsbPads.Connection.claims.get());

        // Once forwarding fails or ends, UsbDriverService rechecks and reclaims without needing replug.
        UsbDriverService.onForwardingEnded(pad);
        shadowOf(Looper.getMainLooper()).idle();
        UsbPads.await(() -> UsbPads.Connection.claims.get() == 2);
        assertEquals(2, UsbPads.Connection.claims.get());
    }

    @Test
    public void reportsDispatchDirectlyOnCallerThreadWithoutMainLooperAndDropAfterUnclaim() throws Exception {
        UsbDevice pad = UsbPads.dualSense("/dev/bus/usb/001/015");
        UsbDriverService.UsbDriverBinder binder = bind(pad);
        final java.util.concurrent.atomic.AtomicReference<Thread> stateThread = new java.util.concurrent.atomic.AtomicReference<>();
        final java.util.concurrent.atomic.AtomicReference<Thread> touchThread = new java.util.concurrent.atomic.AtomicReference<>();
        final java.util.concurrent.atomic.AtomicReference<Thread> motionThread = new java.util.concurrent.atomic.AtomicReference<>();
        final java.util.concurrent.atomic.AtomicReference<Thread> batteryThread = new java.util.concurrent.atomic.AtomicReference<>();
        final java.util.concurrent.atomic.AtomicInteger stateCount = new java.util.concurrent.atomic.AtomicInteger();
        binder.setListener(new UsbDriverListener() {
            @Override public void reportControllerState(int id, int b, float lx, float ly, float rx, float ry, float lt, float rt) {
                stateThread.set(Thread.currentThread());
                stateCount.incrementAndGet();
            }
            @Override public void reportControllerTouch(int id, byte eventType, int pointerId, float x, float y, float pressure) {
                touchThread.set(Thread.currentThread());
            }
            @Override public void reportControllerMotion(int id, byte motionType, float x, float y, float z) {
                motionThread.set(Thread.currentThread());
            }
            @Override public void reportControllerBattery(int id, byte state, byte percentage) {
                batteryThread.set(Thread.currentThread());
            }
            @Override public void deviceRemoved(AbstractController controller) {}
            @Override public void deviceAdded(AbstractController controller) {}
        });
        UsbPads.Connection.controlGate = new CountDownLatch(1);
        UsbPads.Connection.holdControls = true;
        binder.start();
        try {
            UsbDriverService svc = service.get();
            // Controller 0 is claimed immediately during start(); dispatch from a background reader thread
            // WITHOUT idling Looper.getMainLooper() to prove zero main-looper bounce on the hot path.
            java.util.concurrent.atomic.AtomicReference<Thread> callerThread = new java.util.concurrent.atomic.AtomicReference<>();
            Thread reader = new Thread(() -> {
                callerThread.set(Thread.currentThread());
                svc.reportControllerState(0, 1, 0.5f, 0f, 0f, 0f, 0f, 0f);
                svc.reportControllerTouch(0, (byte) 1, 0, 0.5f, 0.5f, 1f);
                svc.reportControllerMotion(0, (byte) 2, 1f, 2f, 3f);
                svc.reportControllerBattery(0, (byte) 1, (byte) 80);
            }, "usb-reader-test");
            reader.start();
            reader.join(2000);
            assertFalse(reader.isAlive());
            assertSame("reportControllerState must run on reader thread", callerThread.get(), stateThread.get());
            assertSame("reportControllerTouch must run on reader thread", callerThread.get(), touchThread.get());
            assertSame("reportControllerMotion must run on reader thread", callerThread.get(), motionThread.get());
            assertSame("reportControllerBattery must run on reader thread", callerThread.get(), batteryThread.get());
            assertEquals(1, stateCount.get());

            // After detach unclaims the pad, direct calls from the reader thread must be dropped immediately.
            detach(pad);
            shadowOf(Looper.getMainLooper()).idle();
            Thread lateReader = new Thread(() -> svc.reportControllerState(0, 1, 0f, 0f, 0f, 0f, 0f, 0f));
            lateReader.start();
            lateReader.join(2000);
            assertEquals("unclaimed pad reports must be dropped immediately", 1, stateCount.get());
        } finally {
            UsbPads.Connection.controlGate.countDown();
            UsbPads.await(() -> UsbPads.Connection.closes.get() == 1);
        }
    }
}

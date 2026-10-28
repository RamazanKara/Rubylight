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
}

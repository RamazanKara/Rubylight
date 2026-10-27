package com.limelight.binding.input.driver;

import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.hardware.usb.UsbDevice;
import android.hardware.usb.UsbManager;

import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ServiceController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowUsbManager;
import org.robolectric.util.ReflectionHelpers;

import java.util.Arrays;
import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 33, application = Application.class,
        shadows = {UsbPads.Connection.class, com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameSirUsbTest {
    @Test
    public void xboxModesUseTheMatchingDriverAndAndroidHidStaysWithAndroid() {
        Application app = RuntimeEnvironment.getApplication();
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(app);
        for (int vendor : new int[] {0x2f24, 0x3537}) {
            for (boolean one : new boolean[] {false, true}) {
                UsbDevice pad = UsbPads.gameSir("gamesir", vendor, one);
                assertEquals(one, XboxOneController.canClaimDevice(pad));
                assertEquals(!one, Xbox360Controller.canClaimDevice(pad));
                assertTrue(UsbDriverService.shouldClaimDevice(pad, config));
                config.usbDriver = false;
                assertFalse(UsbDriverService.shouldClaimDevice(pad, config));
                config.usbDriver = true;
            }
        }
        UsbDevice android = UsbPads.gameSirAndroid("gamesir-android");
        assertFalse(Xbox360Controller.canClaimDevice(android));
        assertFalse(XboxOneController.canClaimDevice(android));
        assertFalse(UsbDriverService.shouldClaimDevice(android, config));
    }

    @Test
    public void greenModeRequestsPermissionThenDeliversInputAndRumble() throws Exception {
        Application app = RuntimeEnvironment.getApplication();
        UsbPads.Connection.reset();
        UsbPads.Connection.inputReport = new byte[20];
        UsbPads.Connection.inputReport[3] = 0x10; // Xbox 360 A button.
        UsbDevice pad = UsbPads.gameSir("/dev/bus/usb/001/008", 0x3537, false);
        ShadowUsbManager manager = shadowOf((UsbManager) app.getSystemService(Context.USB_SERVICE));
        manager.addOrUpdateUsbDevice(pad, false);
        AtomicInteger opened = new AtomicInteger(), completed = new AtomicInteger();
        UsbPads.Recorder reports = new UsbPads.Recorder();
        ServiceController<UsbDriverService> service = Robolectric.buildService(UsbDriverService.class).create();
        try {
            UsbDriverService.UsbDriverBinder binder = (UsbDriverService.UsbDriverBinder) service.get().onBind(new Intent());
            binder.setListener(reports);
            binder.setStateListener(new UsbDriverService.UsbDriverStateListener() {
                public void onUsbPermissionPromptStarting() { opened.incrementAndGet(); }
                public void onUsbPermissionPromptCompleted() { completed.incrementAndGet(); }
            });
            binder.start();
            assertEquals(1, opened.get());
            assertEquals(0, completed.get());
            assertEquals(0, UsbPads.Connection.claims.get());

            manager.addOrUpdateUsbDevice(pad, true);
            app.sendBroadcast(new Intent("com.limelight.USB_PERMISSION").setPackage(app.getPackageName())
                    .putExtra(UsbManager.EXTRA_DEVICE, pad).putExtra(UsbManager.EXTRA_PERMISSION_GRANTED, true));
            UsbPads.await(() -> reports.has("state"));
            assertEquals(1, completed.get());
            List<AbstractController> controllers = ReflectionHelpers.getField(service.get(), "controllers");
            assertEquals(1, controllers.size());
            assertTrue(controllers.get(0) instanceof Xbox360Controller);
            controllers.get(0).rumble((short) 0xffff, (short) 0x8000);
            byte[] rumble = {0, 8, 0, (byte) 0xff, (byte) 0x80, 0, 0, 0};
            assertTrue(UsbPads.Connection.outputs.stream().anyMatch(bytes -> Arrays.equals(rumble, bytes)));
        } finally {
            service.destroy();
            UsbPads.await(() -> UsbPads.Connection.closes.get() == 1);
        }
    }
}

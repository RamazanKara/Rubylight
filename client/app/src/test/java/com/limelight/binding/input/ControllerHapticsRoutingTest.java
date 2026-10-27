package com.limelight.binding.input;

import android.app.Activity;
import android.app.Application;
import com.limelight.binding.input.driver.AbstractController;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class ControllerHapticsRoutingTest {
    @Test
    public void routesOnlyToReadySlotsAndRetiresOldControllerBeforeSlotReuse() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            NvConnection connection = new NvConnection(null,
                    new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, new PreferenceConfiguration());
            Pad first = new Pad(true), second = new Pad(true), unsupported = new Pad(false);
            ControllerHandler.UsbDeviceContext a = register(handler, first, (short)0);
            ControllerHandler.UsbDeviceContext b = register(handler, second, (short)1);
            ControllerHandler.UsbDeviceContext c = register(handler, unsupported, (short)2);
            try {
                byte[] pcm = {1, 0, 2, 0};
                handler.handleControllerHaptics((short)1, 4, pcm);
                handler.handleControllerHaptics((short)2, 4, pcm);
                handler.handleControllerHaptics((short)15, 4, pcm);
                assertEquals(0, first.received);
                assertEquals(1, second.received);
                assertEquals(0, unsupported.received);
                a.destroy();
                handler.handleControllerHaptics((short)0, 5, pcm);
                assertEquals(0, first.received);
                ControllerHandler.UsbDeviceContext replacement = register(handler, second, (short)0);
                a.destroy(); // Late cleanup must not remove the replacement slot.
                handler.handleControllerHaptics((short)0, 6, pcm);
                assertEquals(2, second.received);
                handler.stop();
                handler.handleControllerHaptics((short)0, 7, pcm);
                assertEquals(2, second.received);
                replacement.destroy();
            }
            finally {
                a.destroy(); b.destroy(); c.destroy();
                handler.destroy();
            }
        }
    }

    private static ControllerHandler.UsbDeviceContext register(ControllerHandler handler, Pad pad, short slot) {
        ControllerHandler.UsbDeviceContext context = handler.new UsbDeviceContext();
        context.device = pad;
        context.assignedControllerNumber = true;
        context.controllerNumber = slot;
        context.sendControllerArrival();
        return context;
    }

    private static class Pad extends AbstractController {
        int received;
        Pad(boolean ready) {
            super(0, null, 0x054c, 0x0ce6);
            capabilities = ready ? MoonBridge.LI_CCAP_HAPTICS_PCM : MoonBridge.LI_CCAP_RUMBLE;
        }
        @Override public void controllerHaptics(int sequence, byte[] pcm) { received++; }
        @Override public boolean start() { return true; }
        @Override public void stop() { }
        @Override public void rumble(short low, short high) { }
        @Override public void rumbleTriggers(short left, short right) { }
    }
}

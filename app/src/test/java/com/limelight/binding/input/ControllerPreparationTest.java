package com.limelight.binding.input;

import android.app.Activity;
import android.app.Application;
import com.limelight.binding.input.driver.AbstractController;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;
import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class ControllerPreparationTest {
    @Test public void preparationDoesNotSelectTheMicrophoneBeforeAudioNegotiation() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            Connection connection = new Connection(73);
            ControllerHandler handler = handler(activity.get(), connection);
            Pad pad = new Pad();
            try {
                handler.deviceAdded(pad);
                assertFalse(handler.prepareControllersForLaunch().isEmpty());
                assertFalse(ReflectionHelpers.callInstanceMethod(connection, "controllerMicrophoneOwnsRoute"));
                assertEquals(1, connection.arrivals);
            } finally { handler.destroy(); }
        }
    }

    @Test public void monitorAndRevokedInputCannotPrepareBindOrRouteControllerAudio() {
        for (int appId : new int[] {2147483505, 73}) {
            checkDeniedControllerAccess(appId, appId == 73 ? 0 : -1);
        }
    }

    @Test public void keyboardOnlyAccessCannotPrepareAnnounceBindOrSendControllerInput() {
        checkDeniedControllerAccess(73, 1 << 12);
    }

    private void checkDeniedControllerAccess(int appId, int permission) {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            Connection connection = new Connection(appId);
            ComputerDetails details = new ComputerDetails(); details.permission = permission;
            ReflectionHelpers.setField(connection, "hostDetails", details);
            assertFalse(connection.canSendControllerInput());
            if (permission == (1 << 12)) assertTrue(connection.canSendInput());
            ControllerHandler handler = handler(activity.get(), connection);
            Pad pad = new Pad();
            try {
                handler.deviceAdded(pad);
                assertEquals("", handler.prepareControllersForLaunch());
                handler.reportControllerState(42, 0, 0, 0, 0, 0, 0, 0);
                ControllerHandler.UsbDeviceContext context = handler.new UsbDeviceContext();
                context.device = pad; context.assignedControllerNumber = true;
                context.sendControllerArrival();
                handler.handleControllerAudio(0x5535, new byte[] {1, 3, 0, 0, 3, 0, 0, 0});
                assertEquals(0, connection.arrivals);
                assertEquals(0, connection.inputs);
                assertEquals(0, pad.audioBindings);
                assertEquals(0, pad.audioPackets);
                context.destroy();
            } finally { handler.destroy(); }
        }
    }

    @Test public void keyboardOnlyAccessCannotUseAnAlreadyBoundControllerAudioRoute() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            Connection connection = new Connection(73);
            ControllerHandler handler = handler(activity.get(), connection);
            Pad pad = new Pad();
            ControllerHandler.UsbDeviceContext context = handler.new UsbDeviceContext();
            context.device = pad; context.assignedControllerNumber = true;
            try {
                context.sendControllerArrival();
                byte[] control = {1, 3, 0, 0, 3, 0, 0, 0};
                handler.handleControllerAudio(0x5535, control);
                handler.handleControllerHaptics((short) 0, 0, new byte[4]);
                assertTrue(handler.routeHeadphoneAudio(new short[2], 2, 1));
                assertEquals(1, pad.audioPackets);
                assertEquals(1, pad.headphonePackets);
                assertEquals(1, pad.waveforms);

                ComputerDetails keyboardOnly = new ComputerDetails(); keyboardOnly.permission = 1 << 12;
                ReflectionHelpers.setField(connection, "hostDetails", keyboardOnly);
                context.sendControllerArrival();
                handler.handleControllerAudio(0x5535, control);
                handler.handleControllerHaptics((short) 0, 1, new byte[4]);
                assertFalse(handler.routeHeadphoneAudio(new short[2], 2, 1));
                assertEquals(1, connection.arrivals);
                assertEquals(1, pad.audioBindings);
                assertEquals(1, pad.audioPackets);
                assertEquals(1, pad.headphonePackets);
                assertEquals(1, pad.waveforms);
            } finally { context.destroy(); handler.destroy(); }
        }
    }

    private static ControllerHandler handler(Activity activity, Connection connection) {
        PreferenceConfiguration prefs = new PreferenceConfiguration(); prefs.usbDualSenseHaptics = true;
        return new ControllerHandler(activity, connection, null, prefs);
    }
    private static class Connection extends NvConnection {
        int arrivals, inputs;
        Connection(int appId) {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().setApp(new NvApp("Test", appId, false)).build(), null, null);
        }
        @Override public int sendControllerArrivalEvent(byte slot, short mask, byte type, int buttons, short caps) {
            arrivals++; return 0;
        }
        @Override public void sendControllerInput(short slot, short mask, int buttons, byte leftTrigger,
                                                 byte rightTrigger, short leftX, short leftY, short rightX, short rightY) {
            inputs++;
        }
    }
    private static class Pad extends AbstractController {
        int audioBindings, audioPackets, headphonePackets, waveforms;
        Pad() { super(42, null, 0x054c, 0x0ce6); capabilities = MoonBridge.LI_CCAP_HAPTICS_PCM; }
        @Override public void bindAudio(NvConnection connection, int number) { audioBindings++; }
        @Override public void controllerAudio(int kind, byte[] payload) { audioPackets++; }
        @Override public void controllerHaptics(int sequence, byte[] pcm) { waveforms++; }
        @Override public boolean headphoneAudio(short[] samples, int length, float volume) { headphonePackets++; return true; }
        @Override public boolean start() { return true; }
        @Override public void stop() { }
        @Override public void rumble(short low, short high) { }
        @Override public void rumbleTriggers(short left, short right) { }
    }
}

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
import org.robolectric.shadows.ShadowSystemClock;
import java.util.concurrent.TimeUnit;
import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class UsbControllerArrivalTest {
    @Test
    public void rejectedAnnouncementIsRetriedBeforeInputWithoutReservingAnotherSlot() {
        checkRecovery(false);
    }

    @Test
    public void connectionStartedResendsCapabilitiesWithoutWaitingForTheRetryTimer() {
        checkRecovery(true);
    }

    private void checkRecovery(boolean connectionStarted) {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            Connection connection = new Connection();
            PreferenceConfiguration preferences = new PreferenceConfiguration();
            preferences.multiController = true;
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, preferences);
            Pad pad = new Pad();
            try {
                handler.deviceAdded(pad);
                input(handler);
                assertEquals(1, connection.arrivals);
                assertEquals(0, connection.inputs);
                assertEquals(0, pad.audioBindings);
                // A 250 Hz USB input stream must not flood the connection with retries.
                for (int i = 0; i < 10; i++) input(handler);
                assertEquals(1, connection.arrivals);
                connection.ready = true;
                if (connectionStarted) handler.resendControllerArrivals();
                else ShadowSystemClock.advanceBy(1, TimeUnit.SECONDS);
                input(handler);
                assertEquals(2, connection.arrivals);
                assertEquals(1, connection.inputs);
                assertEquals(0, connection.slot);
                assertEquals(1, connection.activeMask);
                assertEquals(MoonBridge.LI_CTYPE_PS, connection.type);
                assertEquals((short)(MoonBridge.LI_CCAP_HAPTICS_PCM | MoonBridge.LI_CCAP_RUMBLE),
                        connection.capabilities);
                assertEquals(1, pad.audioBindings);
                input(handler);
                assertEquals(2, connection.arrivals);
                assertEquals(2, connection.inputs);
                handler.handleControllerHaptics((short)0, 0, new byte[4]);
                handler.handleSetAdaptiveTriggers((short)0, (byte)12, (byte)1, (byte)1,
                        new byte[10], new byte[10]);
                assertEquals(1, pad.waveforms);
                assertEquals(1, pad.triggers);
                handler.stop();
                handler.resendControllerArrivals();
                assertEquals(2, connection.arrivals);
            } finally {
                handler.destroy();
            }
        }
    }

    private static void input(ControllerHandler handler) {
        handler.reportControllerState(42, 0, 0, 0, 0, 0, 0, 0);
    }

    private static class Connection extends NvConnection {
        boolean ready;
        int arrivals, inputs;
        byte slot, type;
        short activeMask, capabilities;
        Connection() {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }
        @Override public int sendControllerArrivalEvent(byte slot, short activeMask, byte type,
                                                       int buttons, short capabilities) {
            arrivals++;
            this.slot = slot; this.activeMask = activeMask; this.type = type; this.capabilities = capabilities;
            return ready ? 0 : -2;
        }
        @Override public void sendControllerInput(short slot, short mask, int buttons, byte leftTrigger,
                                                 byte rightTrigger, short leftX, short leftY, short rightX, short rightY) {
            inputs++;
        }
    }

    private static class Pad extends AbstractController {
        int audioBindings, waveforms, triggers;
        Pad() {
            super(42, null, 0x054c, 0x0ce6);
            type = MoonBridge.LI_CTYPE_PS;
            capabilities = (short)(MoonBridge.LI_CCAP_HAPTICS_PCM | MoonBridge.LI_CCAP_RUMBLE);
        }
        @Override public void bindAudio(NvConnection connection, int number) { audioBindings++; }
        @Override public void controllerHaptics(int sequence, byte[] pcm) { waveforms++; }
        @Override public void setAdaptiveTriggers(byte flags, byte leftType, byte rightType, byte[] left, byte[] right) { triggers++; }
        @Override public boolean start() { return true; }
        @Override public void stop() { }
        @Override public void rumble(short low, short high) { }
        @Override public void rumbleTriggers(short left, short right) { }
    }
}

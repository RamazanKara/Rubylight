package com.limelight.binding.input;

import android.app.Activity;
import android.app.Application;
import com.limelight.binding.input.driver.AbstractController;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowSystemClock;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
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

    @Test
    public void restoredInputPermissionAnnouncesCapabilitiesBeforeFirstInput() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            Connection connection = new Connection();
            connection.ready = true;
            connection.inputAllowed = false;
            PreferenceConfiguration preferences = new PreferenceConfiguration();
            preferences.multiController = true;
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, preferences);
            Pad pad = new Pad();
            try {
                handler.deviceAdded(pad);
                input(handler);
                assertTrue(connection.events.isEmpty());
                assertEquals(0, pad.audioBindings);
                int permissionChecks = connection.permissionChecks;
                for (int i = 0; i < 10; i++) input(handler);
                assertEquals(permissionChecks, connection.permissionChecks);

                // A denied retry must also stay pending and respect the retry interval.
                ShadowSystemClock.advanceBy(1, TimeUnit.SECONDS);
                input(handler);
                assertTrue(connection.events.isEmpty());
                assertEquals(0, pad.audioBindings);
                connection.inputAllowed = true;
                input(handler);
                assertTrue(connection.events.isEmpty());
                assertEquals(0, pad.audioBindings);

                ShadowSystemClock.advanceBy(1, TimeUnit.SECONDS);
                input(handler);
                assertEquals(Arrays.asList("arrival", "input"), connection.events);
                assertEquals(0, connection.slot);
                assertEquals(1, connection.activeMask);
                assertEquals(MoonBridge.LI_CTYPE_PS, connection.type);
                assertEquals((short)(MoonBridge.LI_CCAP_HAPTICS_PCM | MoonBridge.LI_CCAP_RUMBLE),
                        connection.capabilities);
                assertEquals(1, pad.audioBindings);
                input(handler);
                assertEquals(1, connection.arrivals);
                assertEquals(2, connection.inputs);
            } finally {
                handler.destroy();
            }
        }
    }

    @Test
    public void menuReleaseWaitsForArrivalAndStillClearsAnnouncedInput() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            Connection connection = new Connection();
            PreferenceConfiguration preferences = new PreferenceConfiguration();
            preferences.multiController = true;
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, preferences);
            try {
                handler.deviceAdded(new Pad());
                handler.reportControllerState(42, ControllerPacket.A_FLAG, 1, 0, 0, 0, 0, 0);
                connection.ready = true;
                handler.setInputEnabled(false);
                ShadowSystemClock.advanceBy(1, TimeUnit.SECONDS);
                handler.releaseInput();
                assertEquals(1, connection.arrivals);
                assertEquals(0, connection.inputs);

                handler.setInputEnabled(true);
                input(handler);
                assertEquals(Arrays.asList("arrival", "arrival", "input"), connection.events);
                assertEquals(0, connection.lastButtons);
                handler.reportControllerState(42, ControllerPacket.A_FLAG, 1, 0, 0, 0, 0, 0);
                assertEquals(ControllerPacket.A_FLAG, connection.lastButtons);
                handler.setInputEnabled(false);
                // Both USB and default contexts release their shared, announced slot.
                assertEquals(4, connection.inputs);
                assertEquals(0, connection.lastButtons);
                assertEquals(1, connection.lastInputMask);
                assertEquals(2, connection.arrivals);
            } finally {
                handler.destroy();
            }
        }
    }

    @Test
    public void sharedOnscreenInputWaitsForUsbCapabilities() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            Connection connection = new Connection();
            PreferenceConfiguration preferences = new PreferenceConfiguration();
            preferences.multiController = false;
            preferences.onscreenController = true;
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, preferences);
            try {
                handler.deviceAdded(new Pad());
                input(handler);
                connection.ready = true;
                handler.reportOscState(ControllerPacket.B_FLAG,
                        (short) 0, (short) 0, (short) 0, (short) 0, (byte) 0, (byte) 0);
                assertEquals(1, connection.arrivals);
                assertEquals(0, connection.inputs);
                ShadowSystemClock.advanceBy(1, TimeUnit.SECONDS);
                handler.reportOscState(ControllerPacket.B_FLAG,
                        (short) 0, (short) 0, (short) 0, (short) 0, (byte) 0, (byte) 0);
                assertEquals(Arrays.asList("arrival", "arrival", "input"), connection.events);
                assertEquals(MoonBridge.LI_CTYPE_PS, connection.type);
                assertEquals(ControllerPacket.B_FLAG, connection.lastButtons);
            } finally {
                handler.destroy();
            }
        }
    }

    @Test
    public void existingOnscreenInputStillReleasesWhenNewUsbArrivalFails() {
        for (String release : new String[] {"buttonUp", "menu", "removal"}) {
            try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
                Connection connection = new Connection();
                connection.ready = true;
                PreferenceConfiguration preferences = new PreferenceConfiguration();
                preferences.multiController = false;
                preferences.onscreenController = true;
                ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, preferences);
                Pad pad = new Pad();
                try {
                    handler.reportOscState(ControllerPacket.B_FLAG,
                            (short) 12000, (short) 0, (short) 0, (short) 0, (byte) 0, (byte) 0);
                    assertEquals(1, connection.inputs);
                    assertEquals(ControllerPacket.B_FLAG, connection.lastButtons);
                    connection.ready = false;
                    handler.deviceAdded(pad);
                    handler.reportControllerState(42, ControllerPacket.A_FLAG, 1, 0, 0, 0, 0, 0);
                    assertEquals(1, connection.arrivals);
                    assertEquals(1, connection.inputs);

                    if (release.equals("buttonUp")) {
                        handler.reportOscState(0,
                                (short) 0, (short) 0, (short) 0, (short) 0, (byte) 0, (byte) 0);
                    } else {
                        // Lifecycle releases must not retry an expired announcement.
                        ShadowSystemClock.advanceBy(1, TimeUnit.SECONDS);
                        if (release.equals("menu")) handler.setInputEnabled(false);
                        else handler.deviceRemoved(pad);
                    }
                    assertEquals(release, release.equals("menu") ? 3 : 2, connection.inputs);
                    // The pending USB button and axis must not leak into the OSC button-up.
                    assertEquals(release, 0, connection.lastButtons);
                    assertEquals(release, 0, connection.lastLeftX);
                    assertEquals(release, 1, connection.lastInputMask);
                    assertEquals(release, 1, connection.arrivals);
                    assertEquals(release, 0, pad.audioBindings);
                } finally {
                    handler.destroy();
                }
            }
        }
    }

    @Test
    public void removedSlotDoesNotAuthorizeInputBeforeItsReplacementArrival() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            Connection connection = new Connection();
            connection.ready = true;
            PreferenceConfiguration preferences = new PreferenceConfiguration();
            preferences.multiController = true;
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, preferences);
            Pad pad = new Pad();
            try {
                handler.deviceAdded(pad);
                handler.reportControllerState(42, ControllerPacket.A_FLAG, 1, 0, 0, 0, 0, 0);
                handler.deviceRemoved(pad);
                assertEquals(2, connection.inputs);
                assertEquals(0, connection.lastInputMask);

                connection.ready = false;
                handler.deviceAdded(new Pad());
                input(handler);
                handler.setInputEnabled(false);
                ShadowSystemClock.advanceBy(1, TimeUnit.SECONDS);
                handler.releaseInput();
                assertEquals(2, connection.arrivals);
                assertEquals(2, connection.inputs);
            } finally {
                handler.destroy();
            }
        }
    }

    @Test
    public void sharedUsbRemovalOnlySendsNeutralAfterAnnouncement() {
        for (boolean announced : new boolean[] {false, true}) {
            try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
                Connection connection = new Connection();
                connection.ready = announced;
                PreferenceConfiguration preferences = new PreferenceConfiguration();
                preferences.multiController = false;
                ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, preferences);
                Pad pad = new Pad();
                try {
                    handler.deviceAdded(pad);
                    handler.reportControllerState(42, ControllerPacket.A_FLAG, 1, 0, 0, 0, 0, 0);
                    connection.ready = true;
                    ShadowSystemClock.advanceBy(1, TimeUnit.SECONDS);
                    handler.deviceRemoved(pad);
                    assertEquals(announced ? 2 : 0, connection.inputs);
                    if (announced) {
                        assertEquals(1, connection.lastInputMask);
                        assertEquals(0, connection.lastButtons);
                    }
                    handler.stop();
                    handler.resendControllerArrivals();
                    handler.releaseInput();
                    assertEquals(1, connection.arrivals);
                    assertEquals(announced ? 1 : 0, pad.audioBindings);
                } finally {
                    handler.destroy();
                }
            }
        }
    }

    @Test
    public void pendingReservedUsbRemovalStillClearsTheHostPreparedSlot() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            Connection connection = new Connection();
            PreferenceConfiguration preferences = new PreferenceConfiguration();
            preferences.multiController = true;
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, preferences);
            Pad pad = new Pad();
            try {
                handler.deviceAdded(pad);
                input(handler);
                connection.ready = true;
                ShadowSystemClock.advanceBy(1, TimeUnit.SECONDS);
                handler.deviceRemoved(pad);
                assertEquals(1, connection.inputs);
                assertEquals(0, connection.lastInputMask);
                assertEquals(0, connection.lastButtons);
                assertEquals(1, connection.arrivals);
                assertEquals(0, pad.audioBindings);
            } finally {
                handler.destroy();
            }
        }
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
                assertEquals(connectionStarted ? 2 : 1, connection.inputs);
                assertEquals(0, connection.slot);
                assertEquals(1, connection.activeMask);
                assertEquals(MoonBridge.LI_CTYPE_PS, connection.type);
                assertEquals((short)(MoonBridge.LI_CCAP_HAPTICS_PCM | MoonBridge.LI_CCAP_RUMBLE),
                        connection.capabilities);
                assertEquals(1, pad.audioBindings);
                input(handler);
                assertEquals(2, connection.arrivals);
                assertEquals(connectionStarted ? 3 : 2, connection.inputs);
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
        boolean inputAllowed = true;
        int arrivals, inputs, permissionChecks, lastButtons;
        final List<String> events = new ArrayList<>();
        byte slot, type;
        short activeMask, capabilities, lastInputMask, lastLeftX;
        Connection() {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }
        @Override public boolean canSendInput() {
            permissionChecks++;
            return inputAllowed;
        }
        @Override public int sendControllerArrivalEvent(byte slot, short activeMask, byte type,
                                                       int buttons, short capabilities) {
            arrivals++;
            events.add("arrival");
            this.slot = slot; this.activeMask = activeMask; this.type = type; this.capabilities = capabilities;
            return ready ? 0 : -2;
        }
        @Override public void sendControllerInput(short slot, short mask, int buttons, byte leftTrigger,
                                                 byte rightTrigger, short leftX, short leftY, short rightX, short rightY) {
            inputs++;
            lastInputMask = mask;
            lastButtons = buttons;
            lastLeftX = leftX;
            events.add("input");
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

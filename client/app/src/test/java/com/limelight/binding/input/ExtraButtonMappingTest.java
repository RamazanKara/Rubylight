package com.limelight.binding.input;

import android.app.Activity;
import android.app.Application;
import android.content.Context;
import android.view.KeyEvent;

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
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.*;

/** Extra buttons such as a Razer Kishi Ultra's C and Z mapped to the extended buttons the host passes through. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class ExtraButtonMappingTest {
    private static class Connection extends NvConnection {
        final List<Integer> buttons = new ArrayList<>();
        int arrivals, arrivalButtons, motions, touches;
        byte arrivalType;
        short arrivalCaps;

        Connection() {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }

        @Override public boolean canSendInput() { return true; }

        @Override public int sendControllerArrivalEvent(byte slot, short activeMask, byte type,
                                                       int buttons, short capabilities) {
            arrivals++;
            arrivalType = type;
            arrivalButtons = buttons;
            arrivalCaps = capabilities;
            return 0;
        }

        @Override public int sendControllerMotionEvent(byte number, byte motionType, float x, float y, float z) {
            motions++;
            return 0;
        }

        @Override public int sendControllerTouchEvent(byte number, byte eventType, int pointerId,
                                                      float x, float y, float pressure) {
            touches++;
            return 0;
        }

        @Override public void sendControllerInput(short number, short mask, int buttons, byte leftTrigger,
                                                   byte rightTrigger, short leftX, short leftY, short rightX, short rightY) {
            this.buttons.add(buttons);
        }

        int last() { return buttons.get(buttons.size() - 1); }
    }

    private static class XboxPad extends AbstractController {
        XboxPad() {
            this(0);
        }
        XboxPad(int extraButtons) {
            super(42, null, 0x045e, 0x028e);
            type = MoonBridge.LI_CTYPE_XBOX;
            supportedButtonFlags = ControllerPacket.A_FLAG | ControllerPacket.B_FLAG | extraButtons;
        }
        XboxPad(short caps) {
            this(0);
            capabilities = caps;
        }
        final List<Short> motionRates = new ArrayList<>();
        @Override public void setMotionEventState(byte motionType, short reportRateHz) { motionRates.add(reportRateHz); }
        @Override public boolean start() { return true; }
        @Override public void stop() { }
        @Override public void rumble(short low, short high) { }
        @Override public void rumbleTriggers(short left, short right) { }
    }

    private static KeyEvent key(int action, int keyCode, long time) {
        return new KeyEvent(1000, time, action, keyCode, 0, 0, 0, 0);
    }

    @Test
    public void mappedExtraKeysPressTouchpadMiscAndPaddlesInTheInputPacket() {
        ControllerInputReleaseTest.MenuGame activity = Robolectric.buildActivity(ControllerInputReleaseTest.MenuGame.class).get();
        Connection connection = new Connection();
        ControllerHandler handler = new ControllerHandler(activity, connection, null, new PreferenceConfiguration());
        ControllerHandler.InputDeviceContext context = ReflectionHelpers.getField(handler, "defaultContext");
        context.assignedControllerNumber = true;
        ControllerButtonMap map = new ControllerButtonMap();
        map.put(KeyEvent.KEYCODE_BUTTON_C, ControllerButtonMap.TARGET_TOUCHPAD);
        map.put(KeyEvent.KEYCODE_BUTTON_Z, ControllerButtonMap.TARGET_MISC);
        int[] paddleKeys = {KeyEvent.KEYCODE_BUTTON_13, KeyEvent.KEYCODE_BUTTON_14,
                KeyEvent.KEYCODE_BUTTON_15, KeyEvent.KEYCODE_BUTTON_16};
        int[] paddleTargets = {ControllerButtonMap.TARGET_PADDLE1, ControllerButtonMap.TARGET_PADDLE2,
                ControllerButtonMap.TARGET_PADDLE3, ControllerButtonMap.TARGET_PADDLE4};
        int[] paddleFlags = {ControllerPacket.PADDLE1_FLAG, ControllerPacket.PADDLE2_FLAG,
                ControllerPacket.PADDLE3_FLAG, ControllerPacket.PADDLE4_FLAG};
        for (int i = 0; i < 4; i++) map.put(paddleKeys[i], paddleTargets[i]);
        context.buttonMap = map;
        try {
            assertTrue(handler.handleButtonDown(key(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_C, 1000)));
            assertEquals(ControllerPacket.TOUCHPAD_FLAG, connection.last());
            assertTrue(handler.handleButtonDown(key(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_Z, 1010)));
            assertEquals(ControllerPacket.TOUCHPAD_FLAG | ControllerPacket.MISC_FLAG, connection.last());
            assertTrue(handler.handleButtonUp(key(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_C, 1200)));
            assertEquals(ControllerPacket.MISC_FLAG, connection.last());
            assertTrue(handler.handleButtonUp(key(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_Z, 1210)));
            assertEquals(0, connection.last());

            for (int i = 0; i < 4; i++) {
                assertTrue(handler.handleButtonDown(key(KeyEvent.ACTION_DOWN, paddleKeys[i], 2000 + i)));
                assertEquals(paddleFlags[i], connection.last());
                assertTrue(handler.handleButtonUp(key(KeyEvent.ACTION_UP, paddleKeys[i], 2100 + i)));
                assertEquals(0, connection.last());
            }

            // Without a mapping, C stays unhandled as before.
            context.buttonMap = new ControllerButtonMap();
            int sent = connection.buttons.size();
            assertFalse(handler.handleButtonDown(key(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_C, 3000)));
            assertEquals(sent, connection.buttons.size());
        } finally {
            handler.destroy();
        }
    }

    @Test
    public void usbPadMappedToTheTouchpadArrivesAsPlayStationWithItsExtendedButtons() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            ControllerButtonMap stored = new ControllerButtonMap();
            stored.put(KeyEvent.KEYCODE_BUTTON_A, ControllerButtonMap.TARGET_TOUCHPAD);
            stored.put(KeyEvent.KEYCODE_BUTTON_B, ControllerButtonMap.TARGET_PADDLE3);
            stored.save(activity.get(), ControllerButtonMap.deviceKey(0x045e, 0x028e, null));

            Connection connection = new Connection();
            PreferenceConfiguration preferences = new PreferenceConfiguration();
            preferences.multiController = true;
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, preferences);
            try {
                handler.deviceAdded(new XboxPad());
                handler.reportControllerState(42, ControllerPacket.A_FLAG, 0, 0, 0, 0, 0, 0);
                assertEquals(1, connection.arrivals);
                assertEquals(MoonBridge.LI_CTYPE_PS, connection.arrivalType);
                assertEquals(ControllerPacket.A_FLAG | ControllerPacket.B_FLAG |
                        ControllerPacket.TOUCHPAD_FLAG | ControllerPacket.PADDLE3_FLAG, connection.arrivalButtons);
                assertEquals(ControllerPacket.TOUCHPAD_FLAG, connection.last());
                handler.reportControllerState(42, ControllerPacket.B_FLAG, 0, 0, 0, 0, 0, 0);
                assertEquals(ControllerPacket.PADDLE3_FLAG, connection.last());
            } finally {
                handler.destroy();
                activity.get().getSharedPreferences("ControllerButtonMaps", Context.MODE_PRIVATE).edit().clear().commit();
            }
        }
    }

    @Test
    public void usbShareMappedToTheTouchpadPressesTheTouchpad() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            ControllerButtonMap stored = new ControllerButtonMap();
            stored.put(KeyEvent.KEYCODE_MEDIA_RECORD, ControllerButtonMap.TARGET_TOUCHPAD);
            stored.save(activity.get(), ControllerButtonMap.deviceKey(0x045e, 0x028e, null));

            Connection connection = new Connection();
            PreferenceConfiguration preferences = new PreferenceConfiguration();
            preferences.multiController = true;
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, preferences);
            try {
                handler.deviceAdded(new XboxPad(ControllerPacket.MISC_FLAG));
                handler.reportControllerState(42, ControllerPacket.MISC_FLAG, 0, 0, 0, 0, 0, 0);
                assertEquals(MoonBridge.LI_CTYPE_PS, connection.arrivalType);
                assertEquals(ControllerPacket.TOUCHPAD_FLAG, connection.last());
                handler.reportControllerState(42, ControllerPacket.MISC_FLAG | ControllerPacket.A_FLAG, 0, 0, 0, 0, 0, 0);
                assertEquals(ControllerPacket.TOUCHPAD_FLAG | ControllerPacket.A_FLAG, connection.last());
                handler.reportControllerState(42, 0, 0, 0, 0, 0, 0, 0);
                assertEquals(0, connection.last());
            } finally {
                handler.destroy();
                activity.get().getSharedPreferences("ControllerButtonMaps", Context.MODE_PRIVATE).edit().clear().commit();
            }
        }
    }

    @Test
    public void aTouchpadMappingTurnsOffTheSelectLbClickpadCombo() {
        ControllerInputReleaseTest.MenuGame activity = Robolectric.buildActivity(ControllerInputReleaseTest.MenuGame.class).get();
        Connection connection = new Connection();
        ControllerHandler handler = new ControllerHandler(activity, connection, null, new PreferenceConfiguration());
        ControllerHandler.InputDeviceContext context = ReflectionHelpers.getField(handler, "defaultContext");
        context.assignedControllerNumber = true;
        context.hasSelect = true;
        try {
            // Without a touchpad mapping a pad faked as PlayStation clicks the touchpad with Select+LB.
            assertTrue(ControllerHandler.needsClickpadEmulation(context.buttonMap));
            context.needsClickpadEmulation = true;
            assertTrue(handler.handleButtonDown(key(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_SELECT, 1000)));
            assertTrue(handler.handleButtonDown(key(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_L1, 1010)));
            assertEquals(ControllerPacket.TOUCHPAD_FLAG, connection.last());
            assertTrue(handler.handleButtonUp(key(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_L1, 1100)));
            assertTrue(handler.handleButtonUp(key(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_SELECT, 1110)));
            assertEquals(0, connection.last());

            ControllerButtonMap map = new ControllerButtonMap();
            map.put(KeyEvent.KEYCODE_BUTTON_C, ControllerButtonMap.TARGET_TOUCHPAD);
            context.buttonMap = map;
            assertFalse(ControllerHandler.needsClickpadEmulation(map));
            context.needsClickpadEmulation = ControllerHandler.needsClickpadEmulation(map);
            assertTrue(handler.handleButtonDown(key(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_SELECT, 2000)));
            assertTrue(handler.handleButtonDown(key(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_L1, 2010)));
            assertEquals(ControllerPacket.BACK_FLAG | ControllerPacket.LB_FLAG, connection.last());
            assertTrue(handler.handleButtonDown(key(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_C, 2020)));
            assertEquals(ControllerPacket.BACK_FLAG | ControllerPacket.LB_FLAG | ControllerPacket.TOUCHPAD_FLAG, connection.last());
        } finally {
            handler.destroy();
        }
    }

    @Test
    public void usbShareKeepsItsFlagUnlessMapped() {
        ControllerButtonMap none = new ControllerButtonMap();
        assertEquals(ControllerPacket.MISC_FLAG, none.mapButtonFlags(ControllerPacket.MISC_FLAG, false));
        ControllerButtonMap swap = new ControllerButtonMap();
        swap.put(KeyEvent.KEYCODE_MEDIA_RECORD, KeyEvent.KEYCODE_BUTTON_A);
        swap.put(KeyEvent.KEYCODE_BUTTON_A, ControllerButtonMap.TARGET_MISC);
        assertEquals(ControllerPacket.A_FLAG, swap.mapButtonFlags(ControllerPacket.MISC_FLAG, false));
        assertEquals(ControllerPacket.MISC_FLAG, swap.mapButtonFlags(ControllerPacket.A_FLAG, false));
        ControllerButtonMap off = new ControllerButtonMap();
        off.put(KeyEvent.KEYCODE_MEDIA_RECORD, ControllerButtonMap.DISABLED);
        assertEquals(0, off.mapButtonFlags(ControllerPacket.MISC_FLAG, false));
    }

    @Test
    public void onlyATouchpadMappingChangesTheArrivalType() {
        ControllerButtonMap none = new ControllerButtonMap();
        assertEquals(MoonBridge.LI_CTYPE_XBOX, ControllerHandler.arrivalType(MoonBridge.LI_CTYPE_XBOX, none));
        ControllerButtonMap misc = new ControllerButtonMap();
        misc.put(KeyEvent.KEYCODE_BUTTON_Z, ControllerButtonMap.TARGET_MISC);
        misc.put(KeyEvent.KEYCODE_BUTTON_C, ControllerButtonMap.TARGET_PADDLE1);
        // Share and Capture exist on the Xbox and Switch pads hosts create; paddles choose no type.
        assertEquals(MoonBridge.LI_CTYPE_NINTENDO, ControllerHandler.arrivalType(MoonBridge.LI_CTYPE_NINTENDO, misc));
        assertEquals(MoonBridge.LI_CTYPE_XBOX, ControllerHandler.arrivalType(MoonBridge.LI_CTYPE_XBOX, misc));
        misc.put(KeyEvent.KEYCODE_BUTTON_1, ControllerButtonMap.TARGET_TOUCHPAD);
        assertEquals(MoonBridge.LI_CTYPE_PS, ControllerHandler.arrivalType(MoonBridge.LI_CTYPE_UNKNOWN, misc));
        assertEquals(MoonBridge.LI_CTYPE_PS, ControllerHandler.arrivalType(MoonBridge.LI_CTYPE_PS, misc));
    }

    @Test
    public void aPinnedTypeBeatsTheTouchpadRuleAndTheMotionOverride() {
        ControllerButtonMap touchpad = new ControllerButtonMap();
        touchpad.put(KeyEvent.KEYCODE_BUTTON_C, ControllerButtonMap.TARGET_TOUCHPAD);
        touchpad.setConnectAs(ControllerButtonMap.CONNECT_AS_XBOX);
        assertEquals(MoonBridge.LI_CTYPE_XBOX, ControllerHandler.arrivalType(MoonBridge.LI_CTYPE_UNKNOWN, touchpad));
        // A Razer Kishi: unknown to SDL, motion from the tablet's sensors.
        assertEquals(MoonBridge.LI_CTYPE_XBOX,
                ControllerHandler.inputDeviceArrivalType(MoonBridge.LI_CTYPE_UNKNOWN, true, touchpad));
        assertFalse(ControllerHandler.motionOverridesType(MoonBridge.LI_CTYPE_UNKNOWN, true, touchpad));

        ControllerButtonMap nintendo = new ControllerButtonMap();
        nintendo.setConnectAs(ControllerButtonMap.CONNECT_AS_NINTENDO);
        assertEquals(MoonBridge.LI_CTYPE_NINTENDO,
                ControllerHandler.inputDeviceArrivalType(MoonBridge.LI_CTYPE_XBOX, true, nintendo));
        assertEquals(MoonBridge.LI_CTYPE_NINTENDO, ControllerHandler.arrivalType(MoonBridge.LI_CTYPE_PS, nintendo));

        ControllerButtonMap playStation = new ControllerButtonMap();
        playStation.setConnectAs(ControllerButtonMap.CONNECT_AS_PLAYSTATION);
        assertEquals(MoonBridge.LI_CTYPE_PS,
                ControllerHandler.inputDeviceArrivalType(MoonBridge.LI_CTYPE_XBOX, false, playStation));
    }

    @Test
    public void automaticKeepsTheMotionOverrideAndTheTouchpadRule() {
        ControllerButtonMap automatic = new ControllerButtonMap();
        assertEquals(MoonBridge.LI_CTYPE_UNKNOWN,
                ControllerHandler.inputDeviceArrivalType(MoonBridge.LI_CTYPE_XBOX, true, automatic));
        assertTrue(ControllerHandler.motionOverridesType(MoonBridge.LI_CTYPE_XBOX, true, automatic));
        assertEquals(MoonBridge.LI_CTYPE_XBOX,
                ControllerHandler.inputDeviceArrivalType(MoonBridge.LI_CTYPE_XBOX, false, automatic));
        assertEquals(MoonBridge.LI_CTYPE_PS,
                ControllerHandler.inputDeviceArrivalType(MoonBridge.LI_CTYPE_PS, true, automatic));
        assertFalse(ControllerHandler.motionOverridesType(MoonBridge.LI_CTYPE_PS, true, automatic));
        automatic.put(KeyEvent.KEYCODE_BUTTON_C, ControllerButtonMap.TARGET_TOUCHPAD);
        assertEquals(MoonBridge.LI_CTYPE_PS,
                ControllerHandler.inputDeviceArrivalType(MoonBridge.LI_CTYPE_XBOX, true, automatic));
        assertEquals(MoonBridge.LI_CTYPE_PS,
                ControllerHandler.inputDeviceArrivalType(MoonBridge.LI_CTYPE_NINTENDO, false, automatic));
    }

    @Test
    public void usbPadPinnedAsNintendoArrivesAsNintendoDespiteATouchpadMapping() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            ControllerButtonMap stored = new ControllerButtonMap();
            stored.put(KeyEvent.KEYCODE_BUTTON_A, ControllerButtonMap.TARGET_TOUCHPAD);
            stored.setConnectAs(ControllerButtonMap.CONNECT_AS_NINTENDO);
            stored.save(activity.get(), ControllerButtonMap.deviceKey(0x045e, 0x028e, null));

            Connection connection = new Connection();
            PreferenceConfiguration preferences = new PreferenceConfiguration();
            preferences.multiController = true;
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, preferences);
            try {
                handler.deviceAdded(new XboxPad());
                handler.reportControllerState(42, ControllerPacket.A_FLAG, 0, 0, 0, 0, 0, 0);
                assertEquals(1, connection.arrivals);
                assertEquals(MoonBridge.LI_CTYPE_NINTENDO, connection.arrivalType);
                // The mapped extended button is still announced and sent.
                assertEquals(ControllerPacket.A_FLAG | ControllerPacket.B_FLAG | ControllerPacket.TOUCHPAD_FLAG,
                        connection.arrivalButtons);
                assertEquals(ControllerPacket.TOUCHPAD_FLAG, connection.last());
            } finally {
                handler.destroy();
                activity.get().getSharedPreferences("ControllerButtonMaps", Context.MODE_PRIVATE).edit().clear().commit();
            }
        }
    }

    private static final short MOTION_AND_TOUCHPAD = MoonBridge.LI_CCAP_ACCEL | MoonBridge.LI_CCAP_GYRO |
            MoonBridge.LI_CCAP_TOUCHPAD | MoonBridge.LI_CCAP_DUAL_TOUCHPAD;

    @Test
    public void anXboxPinLeavesOutMotionAndTouchpadSoHostsKeepItXbox() {
        short all = (short) (MOTION_AND_TOUCHPAD | MoonBridge.LI_CCAP_RUMBLE | MoonBridge.LI_CCAP_BATTERY_STATE);
        ControllerButtonMap automatic = new ControllerButtonMap();
        assertEquals(all, ControllerHandler.arrivalCapabilities(all, automatic));
        for (byte kept : new byte[] {ControllerButtonMap.CONNECT_AS_PLAYSTATION, ControllerButtonMap.CONNECT_AS_NINTENDO}) {
            ControllerButtonMap map = new ControllerButtonMap();
            map.setConnectAs(kept);
            assertEquals(all, ControllerHandler.arrivalCapabilities(all, map));
        }
        ControllerButtonMap xbox = new ControllerButtonMap();
        xbox.setConnectAs(ControllerButtonMap.CONNECT_AS_XBOX);
        assertEquals(MoonBridge.LI_CCAP_RUMBLE | MoonBridge.LI_CCAP_BATTERY_STATE,
                ControllerHandler.arrivalCapabilities(all, xbox));
    }

    @Test
    public void usbPadPinnedAsXboxArrivesAndStaysWithoutMotionAndTouchpad() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            ControllerButtonMap stored = new ControllerButtonMap();
            stored.setConnectAs(ControllerButtonMap.CONNECT_AS_XBOX);
            stored.save(activity.get(), ControllerButtonMap.deviceKey(0x045e, 0x028e, null));

            Connection connection = new Connection();
            PreferenceConfiguration preferences = new PreferenceConfiguration();
            preferences.multiController = true;
            preferences.gamepadMotionSensors = true;
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, preferences);
            XboxPad pad = new XboxPad((short) (MOTION_AND_TOUCHPAD | MoonBridge.LI_CCAP_RUMBLE));
            try {
                handler.deviceAdded(pad);
                handler.reportControllerState(42, ControllerPacket.A_FLAG, 0, 0, 0, 0, 0, 0);
                assertEquals(MoonBridge.LI_CTYPE_XBOX, connection.arrivalType);
                assertEquals(MoonBridge.LI_CCAP_RUMBLE, connection.arrivalCaps);
                // The pad's sensors stay off and its motion and touches never reach the host.
                handler.handleSetMotionEventState((short) 0, MoonBridge.LI_MOTION_TYPE_GYRO, (short) 100);
                assertEquals(java.util.Collections.singletonList((short) 0), pad.motionRates);
                handler.reportControllerMotion(42, MoonBridge.LI_MOTION_TYPE_GYRO, 1, 2, 3);
                handler.reportControllerTouch(42, MoonBridge.LI_TOUCH_EVENT_DOWN, 0, 0.5f, 0.5f, 1);
                assertEquals(0, connection.motions);
                assertEquals(0, connection.touches);
            } finally {
                handler.destroy();
                activity.get().getSharedPreferences("ControllerButtonMaps", Context.MODE_PRIVATE).edit().clear().commit();
            }
        }
    }

    @Test
    public void usbPadPinnedAsPlayStationSendsTheFirstButtonThenClicksTheTouchpadWithSelectAndLb() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            ControllerButtonMap stored = new ControllerButtonMap();
            stored.setConnectAs(ControllerButtonMap.CONNECT_AS_PLAYSTATION);
            stored.save(activity.get(), ControllerButtonMap.deviceKey(0x045e, 0x028e, null));

            Connection connection = new Connection();
            PreferenceConfiguration preferences = new PreferenceConfiguration();
            preferences.multiController = true;
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, preferences);
            try {
                handler.deviceAdded(new XboxPad());
                int back = ControllerPacket.BACK_FLAG, lb = ControllerPacket.LB_FLAG;
                // The first button of the combo goes through: holding it back would add latency.
                handler.reportControllerState(42, back, 0, 0, 0, 0, 0, 0);
                assertEquals(MoonBridge.LI_CTYPE_PS, connection.arrivalType);
                assertEquals(back, connection.last());
                handler.reportControllerState(42, back | lb, 0, 0, 0, 0, 0, 0);
                assertEquals(ControllerPacket.TOUCHPAD_FLAG, connection.last());
                handler.reportControllerState(42, back | lb | ControllerPacket.A_FLAG, 0, 0, 0, 0, 0, 0);
                assertEquals(ControllerPacket.TOUCHPAD_FLAG | ControllerPacket.A_FLAG, connection.last());
                // Letting go of LB lifts the click; Back stays silent until it is let go too.
                handler.reportControllerState(42, back, 0, 0, 0, 0, 0, 0);
                assertEquals(0, connection.last());
                handler.reportControllerState(42, 0, 0, 0, 0, 0, 0, 0);
                assertEquals(0, connection.last());
                handler.reportControllerState(42, back, 0, 0, 0, 0, 0, 0);
                assertEquals(back, connection.last());
            } finally {
                handler.destroy();
                activity.get().getSharedPreferences("ControllerButtonMaps", Context.MODE_PRIVATE).edit().clear().commit();
            }
        }
    }
}

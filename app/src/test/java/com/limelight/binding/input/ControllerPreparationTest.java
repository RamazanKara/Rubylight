package com.limelight.binding.input;

import android.app.Activity;
import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.hardware.usb.UsbManager;
import android.hardware.input.InputManager;
import android.view.InputDevice;
import android.view.MotionEvent;
import com.limelight.binding.input.driver.AbstractController;
import com.limelight.binding.input.driver.UsbDriverService;
import com.limelight.binding.input.driver.UsbPads;
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
import org.robolectric.android.controller.ServiceController;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.shadows.ShadowUsbManager;
import org.robolectric.shadows.ShadowInputDevice;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.util.ReflectionHelpers;
import com.limelight.nvstream.input.ControllerPacket;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = {ControllerPreparationTest.ControllerBridge.class, UsbPads.Connection.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class ControllerPreparationTest {
    @Implements(value = MoonBridge.class, isInAndroidSdk = false)
    public static class ControllerBridge {
        @Implementation protected static void __staticInitializer__() { }
        @Implementation protected static boolean guessControllerHasPaddles(int vendor, int product) { return false; }
        @Implementation protected static boolean guessControllerHasShareButton(int vendor, int product) { return false; }
        @Implementation protected static byte guessControllerType(int vendor, int product) { return MoonBridge.LI_CTYPE_UNKNOWN; }
    }

    @Test public void builtInAndExternalPadsArePreparedBeforeAnyInputWithoutUsbHaptics() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, true, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            addInputDevice(activity.get(), 4, true, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            addInputDevice(activity.get(), 8, false, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            addInputDevice(activity.get(), 9, true, InputDevice.SOURCE_DPAD);
            Connection connection = new Connection(73);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.multiController = true;
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                String[] prepared = handler.prepareControllersForLaunch().split(",");
                assertEquals(2, prepared.length);
                assertTrue(prepared[0].startsWith("0:1:"));
                assertTrue(prepared[1].startsWith("1:1:"));
                assertEquals(2, connection.arrivals);
                assertEquals(0, connection.inputs);
                handler.resendControllerArrivals();
                assertEquals(4, connection.arrivals);
                assertEquals(3, connection.inputs);
                assertEquals(3, connection.activeMask & 3);
                java.util.Map<Integer, ControllerHandler.InputDeviceContext> contexts =
                        ReflectionHelpers.getField(handler, "inputDeviceContexts");
                // Built-in controls share player 1 with the first external pad, as upstream.
                assertEquals(0, contexts.get(8).controllerNumber);
                assertFalse(contexts.get(8).reservedControllerNumber);
                assertEquals(0, contexts.get(2).controllerNumber);
                assertEquals(1, contexts.get(4).controllerNumber);
                assertFalse(contexts.containsKey(9));
            } finally { handler.destroy(); }
        }
    }

    @Test public void builtInControlsAloneStayActiveAsPlayerOne() {
        // A handheld whose built-in controls have no sticks: player 1 must stay in the
        // active mask, or hosts drop every packet.
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 1, false, InputDevice.SOURCE_GAMEPAD | InputDevice.SOURCE_DPAD);
            Connection connection = new Connection(73);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.multiController = true;
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                handler.resendControllerArrivals();
                java.util.Map<Integer, ControllerHandler.InputDeviceContext> contexts =
                        ReflectionHelpers.getField(handler, "inputDeviceContexts");
                // What a first button press does (Robolectric's KeyEvent has no device).
                ControllerHandler.InputDeviceContext builtIn = ReflectionHelpers.callInstanceMethod(handler,
                        "createInputDeviceContextForDevice",
                        ReflectionHelpers.ClassParameter.from(InputDevice.class, InputDevice.getDevice(1)));
                contexts.put(1, builtIn);
                ReflectionHelpers.callInstanceMethod(handler, "assignControllerNumberIfNeeded",
                        ReflectionHelpers.ClassParameter.from(ControllerHandler.GenericControllerContext.class, builtIn));
                builtIn.inputMap = ControllerPacket.A_FLAG;
                ReflectionHelpers.callInstanceMethod(handler, "sendControllerInputPacket",
                        ReflectionHelpers.ClassParameter.from(ControllerHandler.GenericControllerContext.class, builtIn));
                assertEquals(0, contexts.get(1).controllerNumber);
                assertFalse(contexts.get(1).reservedControllerNumber);
                assertTrue(connection.inputs > 0);
                assertEquals(1, connection.activeMask & 1);
                handler.onInputDeviceRemoved(1);
                assertEquals(0, connection.activeMask);
            } finally { handler.destroy(); }
        }
    }

    @Test public void aHostWithoutLaunchPreparationGetsAttachedAndNewPadsAtConnectionStart() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, true, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            Connection connection = new Connection(73);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.multiController = true;
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                handler.resendControllerArrivals();
                assertEquals(1, connection.arrivals);
                assertEquals(1, connection.inputs);
                addInputDevice(activity.get(), 3, true, InputDevice.SOURCE_GAMEPAD);
                handler.onInputDeviceAdded(3);
                assertEquals(2, connection.arrivals);
                assertEquals(2, connection.inputs);
                assertEquals(3, connection.activeMask);
            } finally { handler.destroy(); }
        }
    }

    private static PreferenceConfiguration onscreenOwnPlayerPrefs(Activity activity) {
        // The attached mask reads the saved settings, the handler its session's.
        android.preference.PreferenceManager.getDefaultSharedPreferences(activity).edit()
                .putBoolean("checkbox_multi_controller", true)
                .putBoolean("checkbox_show_onscreen_controls", true)
                .putBoolean("checkbox_onscreen_own_player", true).commit();
        return PreferenceConfiguration.readPreferences(activity);
    }

    private static java.util.Map<Integer, ControllerHandler.InputDeviceContext> contexts(ControllerHandler handler) {
        return ReflectionHelpers.getField(handler, "inputDeviceContexts");
    }

    private static final int JOYPAD = InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD;

    @Test public void onscreenOwnPlayerPutsAttachedPadsAfterIt() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, true, JOYPAD);
            addInputDevice(activity.get(), 4, true, JOYPAD);
            PreferenceConfiguration prefs = onscreenOwnPlayerPrefs(activity.get());
            // What the launch tells the host: controller 1 and the two pads behind it.
            assertEquals(7, ControllerHandler.getAttachedControllerMask(activity.get()));
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                String[] prepared = handler.prepareControllersForLaunch().split(",");
                assertEquals(3, prepared.length);
                assertTrue(prepared[0], prepared[0].startsWith("0:" + MoonBridge.LI_CTYPE_XBOX + ":0:"));
                assertEquals(0, Integer.parseUnsignedInt(prepared[0].split(":")[3]) & ControllerPacket.TOUCHPAD_FLAG);
                assertTrue(prepared[1], prepared[1].startsWith("1:"));
                assertTrue(prepared[2], prepared[2].startsWith("2:"));
                assertEquals(1, contexts(handler).get(2).controllerNumber);
                assertEquals(2, contexts(handler).get(4).controllerNumber);

                connection.events.clear();
                handler.resendControllerArrivals();
                // First to arrive and first with a state, or the PC's own numbering starts with a pad.
                assertEquals("arrival 0", connection.events.get(0));
                assertEquals("input 0 0", connection.events.get(1));
                assertEquals(7, connection.activeMask);
            } finally { handler.destroy(); }
        }
    }

    @Test public void onscreenOwnPlayerKeepsItsNumberThroughAPadLeavingAndComingBack() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            PreferenceConfiguration prefs = onscreenOwnPlayerPrefs(activity.get());
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                handler.resendControllerArrivals();
                assertEquals(1, connection.activeMask);

                // Connected after the stream started.
                addInputDevice(activity.get(), 5, true, JOYPAD);
                handler.onInputDeviceAdded(5);
                assertEquals(1, contexts(handler).get(5).controllerNumber);
                assertEquals(3, connection.activeMask);

                handler.onInputDeviceRemoved(5);
                assertEquals(1, connection.activeMask);

                // Back with a new id: the same player, and no pad ever on controller 1.
                addInputDevice(activity.get(), 6, true, JOYPAD);
                handler.onInputDeviceAdded(6);
                assertEquals(1, contexts(handler).get(6).controllerNumber);
                assertEquals(3, connection.activeMask);

                // The stream menu hiding the controls keeps every number.
                prefs.onscreenController = false;
                handler.reportOscState(0, (short) 0, (short) 0, (short) 0, (short) 0, (byte) 0, (byte) 0);
                assertEquals(3, connection.activeMask);
            } finally { handler.destroy(); }
        }
    }

    @Test public void onscreenOwnPlayerDoesNotMixWithBuiltInOrUsbPads() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, false, JOYPAD);
            PreferenceConfiguration prefs = onscreenOwnPlayerPrefs(activity.get());
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                handler.deviceAdded(new Pad());
                handler.resendControllerArrivals();
                ControllerHandler.InputDeviceContext builtIn = contexts(handler).get(2);
                assertNotEquals(0, builtIn.controllerNumber);
                assertTrue(builtIn.reservedControllerNumber);
                assertEquals(7, connection.activeMask & 7);

                connection.events.clear();
                handler.reportOscState(ControllerPacket.A_FLAG, (short) 0, (short) 0, (short) 0, (short) 0, (byte) 0, (byte) 0);
                assertEquals(Collections.singletonList("input 0 " + ControllerPacket.A_FLAG), connection.events);

                // A pad's state stays on its own number, without the on-screen press.
                connection.events.clear();
                builtIn.inputMap = ControllerPacket.B_FLAG;
                ReflectionHelpers.callInstanceMethod(handler, "sendControllerInputPacket",
                        ReflectionHelpers.ClassParameter.from(ControllerHandler.GenericControllerContext.class, builtIn));
                assertEquals(Collections.singletonList("input " + builtIn.controllerNumber + " " + ControllerPacket.B_FLAG),
                        connection.events);
            } finally { handler.destroy(); }
        }
    }

    @Test public void onscreenOwnPlayerAnnouncesTheTouchpadPadItsButtonNeeds() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, true, JOYPAD);
            PreferenceConfiguration prefs = onscreenOwnPlayerPrefs(activity.get());
            prefs.showTouchpadButton = true;
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                String[] prepared = handler.prepareControllersForLaunch().split(",");
                assertTrue(prepared[0], prepared[0].startsWith("0:" + MoonBridge.LI_CTYPE_PS + ":"));
                assertEquals(ControllerPacket.TOUCHPAD_FLAG,
                        Integer.parseUnsignedInt(prepared[0].split(":")[3]) & ControllerPacket.TOUCHPAD_FLAG);
                assertTrue(prepared[1], prepared[1].startsWith("1:" + MoonBridge.LI_CTYPE_XBOX + ":"));
            } finally { handler.destroy(); }
        }
    }

    @Test public void onscreenOwnPlayerNeedsOnscreenControlsMultipleControllersAndMoreThanL3R3() {
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.onscreenOwnPlayer = prefs.multiController = prefs.onscreenController = true;
        assertTrue(ControllerHandler.onscreenOwnsPlayerOne(prefs));
        prefs.onlyL3R3 = true;
        assertFalse(ControllerHandler.onscreenOwnsPlayerOne(prefs));
        prefs.onlyL3R3 = false;
        prefs.multiController = false;
        assertFalse(ControllerHandler.onscreenOwnsPlayerOne(prefs));
        prefs.multiController = true;
        prefs.onscreenController = false;
        assertFalse(ControllerHandler.onscreenOwnsPlayerOne(prefs));
        prefs.onscreenController = true;
        prefs.onscreenOwnPlayer = false;
        assertFalse(ControllerHandler.onscreenOwnsPlayerOne(prefs));
    }

    @Test public void withoutTheOptionAPadStillSharesPlayerOneWithTheOnscreenControls() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, true, JOYPAD);
            PreferenceConfiguration prefs = onscreenOwnPlayerPrefs(activity.get());
            prefs.onscreenOwnPlayer = false;
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                assertTrue(handler.prepareControllersForLaunch().startsWith("0:"));
                assertEquals(0, contexts(handler).get(2).controllerNumber);
            } finally { handler.destroy(); }
        }
    }

    private static ControllerHandler handheld(Activity activity, Connection connection, PreferenceConfiguration prefs) {
        return new ControllerHandler(activity, connection, null, prefs) {
            @Override protected boolean[] findInternalGamepadAndSelect() { return new boolean[] {true, true, true}; }
        };
    }

    private static android.view.KeyEvent systemBack(int action) {
        return new android.view.KeyEvent(1000, 1000, action, android.view.KeyEvent.KEYCODE_BACK, 0, 0, -1, 0, 0,
                InputDevice.SOURCE_KEYBOARD);
    }

    private static void send(ControllerHandler handler, ControllerHandler.GenericControllerContext context) {
        ReflectionHelpers.callInstanceMethod(handler, "sendControllerInputPacket",
                ReflectionHelpers.ClassParameter.from(ControllerHandler.GenericControllerContext.class, context));
    }

    @Test public void onscreenOwnPlayerSendsAHandheldsBackAsGuideAndDeviceLessInputOnItsBuiltInPad() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, true, JOYPAD);
            addInputDevice(activity.get(), 8, false, JOYPAD);
            PreferenceConfiguration prefs = onscreenOwnPlayerPrefs(activity.get());
            prefs.backAsGuide = true;
            // Guide at once, not held back for Hold Guide to open the stream menu
            prefs.guideMenuHoldMs = 0;
            Connection connection = new Connection(73);
            ControllerHandler handler = handheld(activity.get(), connection, prefs);
            try {
                handler.prepareControllersForLaunch();
                handler.resendControllerArrivals();
                ControllerHandler.InputDeviceContext builtIn = contexts(handler).get(8);
                assertEquals(2, builtIn.controllerNumber);

                connection.events.clear();
                assertTrue(handler.handleHandheldBackAsGuide(systemBack(android.view.KeyEvent.ACTION_DOWN)));
                assertEquals(Collections.singletonList("input 2 " + ControllerPacket.SPECIAL_BUTTON_FLAG), connection.events);
                // A chord with the built-in buttons reaches the PC as one pad.
                builtIn.inputMap = ControllerPacket.RB_FLAG;
                send(handler, builtIn);
                assertEquals("input 2 " + (ControllerPacket.SPECIAL_BUTTON_FLAG | ControllerPacket.RB_FLAG),
                        connection.events.get(connection.events.size() - 1));
                builtIn.inputMap = 0;
                assertTrue(handler.handleHandheldBackAsGuide(systemBack(android.view.KeyEvent.ACTION_UP)));
                assertEquals("input 2 0", connection.events.get(connection.events.size() - 1));
                for (String event : connection.events) assertFalse(event, event.startsWith("input 0 "));

                // Events from no device join the built-in pad too, not the on-screen controls.
                ControllerHandler.InputDeviceContext noDevice = ReflectionHelpers.callInstanceMethod(handler, "getContextForEvent",
                        ReflectionHelpers.ClassParameter.from(android.view.InputEvent.class, new android.view.KeyEvent(0, 0,
                                android.view.KeyEvent.ACTION_DOWN, android.view.KeyEvent.KEYCODE_BUTTON_A, 0, 0, 0, 0)));
                assertNotSame(ReflectionHelpers.getField(handler, "defaultContext"), noDevice);
                assertEquals(2, noDevice.controllerNumber);
            } finally { handler.destroy(); }
        }
    }

    @Test public void onscreenOwnPlayerSendsBackAsGuideOnTheFirstPadNumberWithoutABuiltInPad() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            PreferenceConfiguration prefs = onscreenOwnPlayerPrefs(activity.get());
            prefs.backAsGuide = true;
            prefs.guideMenuHoldMs = 0;
            Connection connection = new Connection(73);
            ControllerHandler handler = handheld(activity.get(), connection, prefs);
            try {
                handler.resendControllerArrivals();
                connection.events.clear();
                assertTrue(handler.handleHandheldBackAsGuide(systemBack(android.view.KeyEvent.ACTION_DOWN)));
                assertEquals(Collections.singletonList("input 1 " + ControllerPacket.SPECIAL_BUTTON_FLAG), connection.events);
                assertTrue(handler.handleHandheldBackAsGuide(systemBack(android.view.KeyEvent.ACTION_UP)));
            } finally { handler.destroy(); }
        }
    }

    @Test public void onscreenOwnPlayerJoinsStickLessBuiltInButtonsToTheBuiltInPad() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, true, JOYPAD);
            addInputDevice(activity.get(), 8, false, JOYPAD);
            // Start, Select and Home on a kernel device of their own, as on some handhelds.
            addInputDevice(activity.get(), 9, false, InputDevice.SOURCE_GAMEPAD | InputDevice.SOURCE_DPAD);
            PreferenceConfiguration prefs = onscreenOwnPlayerPrefs(activity.get());
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                assertEquals(3, handler.prepareControllersForLaunch().split(",").length);
                handler.resendControllerArrivals();
                assertEquals(7, connection.activeMask);
                int arrivals = connection.arrivals;

                // What its first button press does (Robolectric's KeyEvent has no device).
                ControllerHandler.InputDeviceContext buttons = ReflectionHelpers.callInstanceMethod(handler,
                        "createInputDeviceContextForDevice",
                        ReflectionHelpers.ClassParameter.from(InputDevice.class, InputDevice.getDevice(9)));
                contexts(handler).put(9, buttons);
                buttons.inputMap = ControllerPacket.PLAY_FLAG;
                send(handler, buttons);
                assertEquals(contexts(handler).get(8).controllerNumber, buttons.controllerNumber);
                assertFalse(buttons.reservedControllerNumber);
                // No controller more on the PC.
                assertEquals(7, connection.activeMask);
                assertEquals("input 2 " + ControllerPacket.PLAY_FLAG, connection.events.get(connection.events.size() - 1));

                // Gone again, the built-in pad keeps its number.
                handler.onInputDeviceRemoved(9);
                assertEquals(7, connection.activeMask);
                assertTrue(connection.arrivals >= arrivals);
            } finally { handler.destroy(); }
        }
    }

    @Test public void onscreenOwnPlayerGivesStickLessBuiltInButtonsANumberOfTheirOwnWithoutABuiltInPad() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 9, false, InputDevice.SOURCE_GAMEPAD | InputDevice.SOURCE_DPAD);
            PreferenceConfiguration prefs = onscreenOwnPlayerPrefs(activity.get());
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                handler.resendControllerArrivals();
                ControllerHandler.InputDeviceContext buttons = ReflectionHelpers.callInstanceMethod(handler,
                        "createInputDeviceContextForDevice",
                        ReflectionHelpers.ClassParameter.from(InputDevice.class, InputDevice.getDevice(9)));
                contexts(handler).put(9, buttons);
                buttons.inputMap = ControllerPacket.A_FLAG;
                send(handler, buttons);
                // Active on the PC, or hosts drop its packets.
                assertEquals(1, buttons.controllerNumber);
                assertTrue(buttons.reservedControllerNumber);
                assertEquals(3, connection.activeMask);
            } finally { handler.destroy(); }
        }
    }

    @Test public void onscreenOwnPlayerGivesTheDevicesMotorsAndSensorsToTheFirstPadNotTheOnscreenControls() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            android.hardware.SensorManager sensors = (android.hardware.SensorManager)
                    activity.get().getSystemService(Context.SENSOR_SERVICE);
            for (int type : new int[] {android.hardware.Sensor.TYPE_ACCELEROMETER, android.hardware.Sensor.TYPE_GYROSCOPE}) {
                org.robolectric.Shadows.shadowOf(sensors).addSensor(org.robolectric.shadows.ShadowSensor.newInstance(type));
            }
            android.os.Vibrator vibrator = (android.os.Vibrator) activity.get().getSystemService(Context.VIBRATOR_SERVICE);
            addInputDevice(activity.get(), 2, true, JOYPAD);
            addInputDevice(activity.get(), 4, true, JOYPAD);
            PreferenceConfiguration prefs = onscreenOwnPlayerPrefs(activity.get());
            prefs.gamepadMotionSensors = true;
            prefs.gamepadMotionSensorsFallbackToDevice = true;
            prefs.vibrateFallbackToDevice = true;
            prefs.vibrateOsc = true;
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                handler.prepareControllersForLaunch();
                ControllerHandler.InputDeviceContext first = contexts(handler).get(2);
                ControllerHandler.InputDeviceContext second = contexts(handler).get(4);
                assertEquals(1, first.controllerNumber);
                assertEquals(2, second.controllerNumber);
                // Motion: the first pad streams the device's sensors, and its own gyroscope gets the liveness check.
                assertSame(sensors, first.sensorManager);
                assertNotSame(sensors, second.sensorManager);
                assertTrue(handler.checksGyroLiveness((short) 1, null));
                assertFalse(handler.checksGyroLiveness((short) 0, null));
                assertFalse(handler.checksGyroLiveness((short) 2, null));

                // Rumble: the second pad without a motor leaves the phone still.
                handler.handleRumble((short) 2, (short) 0x8000, (short) 0x8000);
                assertFalse(org.robolectric.Shadows.shadowOf(vibrator).isVibrating());
                // The first pad without a motor vibrates the phone, and its removal stops it.
                handler.handleRumble((short) 1, (short) 0x8000, (short) 0x8000);
                assertTrue(org.robolectric.Shadows.shadowOf(vibrator).isVibrating());
                assertTrue(ReflectionHelpers.<Boolean>getField(handler, "deviceVibratorForPlayerOne"));
                handler.onInputDeviceRemoved(2);
                assertFalse(org.robolectric.Shadows.shadowOf(vibrator).isVibrating());
                // Controller 1 has no pad: a game's rumble for it is the on-screen controls' own.
                handler.handleRumble((short) 0, (short) 0x8000, (short) 0x8000);
                assertTrue(org.robolectric.Shadows.shadowOf(vibrator).isVibrating());
                assertFalse(ReflectionHelpers.<Boolean>getField(handler, "deviceVibratorForPlayerOne"));
            } finally { handler.destroy(); }
        }
    }

    @Test public void onscreenOwnPlayerGivesTheDevicesSensorsToAUsbPadOnTheFirstPadNumber() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            android.hardware.SensorManager sensors = (android.hardware.SensorManager)
                    activity.get().getSystemService(Context.SENSOR_SERVICE);
            for (int type : new int[] {android.hardware.Sensor.TYPE_ACCELEROMETER, android.hardware.Sensor.TYPE_GYROSCOPE}) {
                org.robolectric.Shadows.shadowOf(sensors).addSensor(org.robolectric.shadows.ShadowSensor.newInstance(type));
            }
            PreferenceConfiguration prefs = onscreenOwnPlayerPrefs(activity.get());
            prefs.gamepadMotionSensors = true;
            prefs.gamepadMotionSensorsFallbackToDevice = true;
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                Pad first = new Pad(42, (short) 0);
                Pad second = new Pad(43, (short) 0);
                handler.deviceAdded(first);
                handler.deviceAdded(second);
                handler.prepareControllersForLaunch();
                java.util.Map<Integer, ControllerHandler.UsbDeviceContext> usb = ReflectionHelpers.getField(handler, "usbDeviceContexts");
                assertEquals(1, usb.get(42).controllerNumber);
                assertEquals(2, usb.get(43).controllerNumber);
                assertSame(sensors, ReflectionHelpers.getField(usb.get(42), "deviceSensors"));
                assertNull(ReflectionHelpers.getField(usb.get(43), "deviceSensors"));
            } finally { handler.destroy(); }
        }
    }

    @Test public void sharedPlayerOneIsPreparedOnlyOnce() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, true, InputDevice.SOURCE_GAMEPAD);
            addInputDevice(activity.get(), 3, true, InputDevice.SOURCE_GAMEPAD);
            ControllerHandler handler = handler(activity.get(), new Connection(73));
            try {
                handler.deviceAdded(new Pad());
                assertEquals(1, handler.prepareControllersForLaunch().split(",").length);
            } finally { handler.destroy(); }
        }
    }

    @Test public void aBuiltInDeviceWithOnlyGamepadKeysLeavesPlayerOneToTheRealPad() {
        // #29 and #30: a tablet or TV box exposes a built-in device that advertises gamepad
        // keys. It took player 1 at launch, so games ignored the real controller as player 2.
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 1, false, InputDevice.SOURCE_GAMEPAD | InputDevice.SOURCE_KEYBOARD);
            addInputDevice(activity.get(), 2, true, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            Connection connection = new Connection(73);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.multiController = true;
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                String[] prepared = handler.prepareControllersForLaunch().split(",");
                assertEquals(1, prepared.length);
                assertTrue(prepared[0].startsWith("0:"));
                handler.resendControllerArrivals();
                assertEquals(1, connection.activeMask);
                java.util.Map<Integer, ControllerHandler.InputDeviceContext> contexts =
                        ReflectionHelpers.getField(handler, "inputDeviceContexts");
                assertEquals(0, contexts.get(2).controllerNumber);
                assertFalse(contexts.containsKey(1));
                int arrivals = connection.arrivals;
                // Its first gamepad key mid-game shares player 1 and does not re-announce the slot.
                ControllerHandler.InputDeviceContext builtIn = ReflectionHelpers.callInstanceMethod(handler,
                        "createInputDeviceContextForDevice",
                        ReflectionHelpers.ClassParameter.from(InputDevice.class, InputDevice.getDevice(1)));
                contexts.put(1, builtIn);
                ReflectionHelpers.callInstanceMethod(handler, "assignControllerNumberIfNeeded",
                        ReflectionHelpers.ClassParameter.from(ControllerHandler.GenericControllerContext.class, builtIn));
                assertEquals(0, builtIn.controllerNumber);
                assertFalse(builtIn.reservedControllerNumber);
                assertEquals(arrivals, connection.arrivals);
            } finally { handler.destroy(); }
        }
    }

    @Test public void aVirtualBuiltInGamepadDoesNotPushTheRealPadToPlayerTwo() {
        // Even a built-in device with sticks (a vendor game-mode uinput pad) only shares player 1.
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 1, false, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            addInputDevice(activity.get(), 2, true, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.multiController = true;
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                String[] prepared = handler.prepareControllersForLaunch().split(",");
                assertEquals(1, prepared.length);
                assertTrue(prepared[0].startsWith("0:"));
                assertEquals(1, connection.arrivals);
                handler.resendControllerArrivals();
                assertEquals(2, connection.arrivals);
                java.util.Map<Integer, ControllerHandler.InputDeviceContext> contexts =
                        ReflectionHelpers.getField(handler, "inputDeviceContexts");
                assertEquals(0, contexts.get(1).controllerNumber);
                assertEquals(0, contexts.get(2).controllerNumber);
                // When the external pad leaves, the built-in controls announce player 1 again.
                handler.onInputDeviceRemoved(2);
                assertEquals(3, connection.arrivals);
                assertEquals(1, connection.activeMask & 1);
            } finally { handler.destroy(); }
        }
    }

    @Test public void aRealPadGoesAheadOfAnExternalButtonOnlyDevice() {
        // A remote advertising gamepad keys gets a slot after any real pad attached at launch.
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, true, InputDevice.SOURCE_GAMEPAD | InputDevice.SOURCE_DPAD);
            addInputDevice(activity.get(), 5, true, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.multiController = true;
            ControllerHandler handler = new ControllerHandler(activity.get(), new Connection(73), null, prefs);
            try {
                handler.prepareControllersForLaunch();
                java.util.Map<Integer, ControllerHandler.InputDeviceContext> contexts =
                        ReflectionHelpers.getField(handler, "inputDeviceContexts");
                assertEquals(0, contexts.get(5).controllerNumber);
                assertEquals(1, contexts.get(2).controllerNumber);
            } finally { handler.destroy(); }
        }
    }

    @Test public void aPinnedTypeReachesTheLaunchListAndBeatsTheMotionOverrideWithoutAToast() {
        // #53: a Razer Kishi is unknown to SDL and gets motion from the tablet, so Automatic
        // makes the host create a PlayStation pad; the user pins it. Each case loads its pin from storage.
        short motion = MoonBridge.LI_CCAP_ACCEL | MoonBridge.LI_CCAP_GYRO;

        PinnedArrival automatic = arriveWithDeviceMotion(null);
        assertEquals(MoonBridge.LI_CTYPE_UNKNOWN, automatic.type);
        assertTrue(automatic.launch, automatic.launch.startsWith("0:1:")); // the launch list names unknown as Xbox
        assertEquals(motion, automatic.caps & motion);
        assertTrue(automatic.toast);
        assertTrue(automatic.clickpad);
        assertTrue(automatic.deviceSensors);

        // Pinned as Xbox: no motion or touchpad, so hosts that make such pads a DualSense keep it Xbox.
        PinnedArrival xbox = arriveWithDeviceMotion(ControllerButtonMap.CONNECT_AS_XBOX);
        assertEquals(MoonBridge.LI_CTYPE_XBOX, xbox.type);
        assertTrue(xbox.launch, xbox.launch.startsWith("0:1:0:"));
        assertEquals(0, xbox.caps & (motion | MoonBridge.LI_CCAP_TOUCHPAD));
        assertFalse(xbox.toast);
        assertFalse(xbox.clickpad);
        assertFalse(xbox.deviceSensors);

        // Pinned as PlayStation: motion stays, and Select+LB stands in for the missing touchpad click.
        PinnedArrival playStation = arriveWithDeviceMotion(ControllerButtonMap.CONNECT_AS_PLAYSTATION);
        assertEquals(MoonBridge.LI_CTYPE_PS, playStation.type);
        assertTrue(playStation.launch, playStation.launch.startsWith("0:2:"));
        assertEquals(motion, playStation.caps & motion);
        assertFalse(playStation.toast);
        assertTrue(playStation.clickpad);
        assertTrue(playStation.deviceSensors);
    }

    private static final class PinnedArrival {
        String launch;
        byte type;
        short caps;
        boolean toast, clickpad, deviceSensors;
    }

    /** Player 1 without its own sensors, with the device's motion as fallback, and `connectAs` stored for it. */
    private static PinnedArrival arriveWithDeviceMotion(Byte connectAs) {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            android.hardware.SensorManager sensors = (android.hardware.SensorManager)
                    activity.get().getSystemService(Context.SENSOR_SERVICE);
            for (int type : new int[] {android.hardware.Sensor.TYPE_ACCELEROMETER, android.hardware.Sensor.TYPE_GYROSCOPE}) {
                org.robolectric.Shadows.shadowOf(sensors).addSensor(org.robolectric.shadows.ShadowSensor.newInstance(type));
            }
            addInputDevice(activity.get(), 2, true, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            if (connectAs != null) {
                ControllerButtonMap stored = new ControllerButtonMap();
                stored.setConnectAs(connectAs);
                stored.save(activity.get(), ControllerButtonMap.deviceKey(0, 0, "Controller 2"));
            }
            Connection connection = new Connection(73);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.multiController = true;
            prefs.gamepadMotionSensors = true;
            prefs.gamepadMotionSensorsFallbackToDevice = true;
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            // Idling the main looper would also run the Shield service binding; run only what the arrival posts.
            MainThreadPosts posts = new MainThreadPosts();
            ReflectionHelpers.setField(handler, "mainThreadHandler", posts);
            org.robolectric.shadows.ShadowToast.reset();
            try {
                PinnedArrival result = new PinnedArrival();
                String[] prepared = handler.prepareControllersForLaunch().split(",");
                assertEquals(1, prepared.length);
                posts.runAll();
                java.util.Map<Integer, ControllerHandler.InputDeviceContext> contexts =
                        ReflectionHelpers.getField(handler, "inputDeviceContexts");
                ControllerHandler.InputDeviceContext context = contexts.get(2);
                assertEquals(connectAs == null ? ControllerButtonMap.CONNECT_AS_AUTOMATIC : connectAs.byteValue(),
                        context.buttonMap.connectAs());
                result.launch = prepared[0];
                result.type = connection.arrivalType;
                result.caps = connection.arrivalCaps;
                result.toast = org.robolectric.shadows.ShadowToast.getLatestToast() != null;
                result.clickpad = context.needsClickpadEmulation;
                result.deviceSensors = context.sensorManager == sensors;
                assertEquals(Short.parseShort(prepared[0].split(":")[2]), result.caps);
                return result;
            } finally {
                handler.destroy();
                activity.get().getSharedPreferences("ControllerButtonMaps", Context.MODE_PRIVATE).edit().clear().commit();
            }
        }
    }

    /** Keeps what is posted to the main thread, to run it without idling the looper. */
    private static class MainThreadPosts extends android.os.Handler {
        final List<Runnable> posted = new ArrayList<>();
        MainThreadPosts() { super(android.os.Looper.getMainLooper()); }
        @Override public boolean sendMessageAtTime(android.os.Message message, long uptimeMillis) {
            posted.add(message.getCallback()); return true;
        }
        void runAll() {
            List<Runnable> run = new ArrayList<>(posted);
            posted.clear();
            for (Runnable runnable : run) if (runnable != null) runnable.run();
        }
    }

    private static void addInputDevice(Activity activity, int id, boolean external, int sources) {
        addInputDevice(activity, id, external, sources, "Controller " + id, 0, 0);
    }

    private static void addInputDevice(Activity activity, int id, boolean external, int sources,
                                       String name, int vendor, int product) {
        InputDevice device = ShadowInputDevice.makeInputDeviceNamed(name);
        ReflectionHelpers.setField(device, "mVendorId", vendor);
        ReflectionHelpers.setField(device, "mProductId", product);
        ReflectionHelpers.setField(device, "mId", id);
        ReflectionHelpers.setField(device, "mDescriptor", "controller-" + id);
        ReflectionHelpers.setField(device, "mIsExternal", external);
        ReflectionHelpers.setField(device, "mSources", sources);
        ReflectionHelpers.setField(device, "mMotionRanges", new ArrayList<InputDevice.MotionRange>());
        if ((sources & InputDevice.SOURCE_JOYSTICK) == InputDevice.SOURCE_JOYSTICK) {
            for (int axis : new int[] {MotionEvent.AXIS_X, MotionEvent.AXIS_Y}) {
                ReflectionHelpers.callInstanceMethod(device, "addMotionRange",
                        ReflectionHelpers.ClassParameter.from(int.class, axis),
                        ReflectionHelpers.ClassParameter.from(int.class, InputDevice.SOURCE_JOYSTICK),
                        ReflectionHelpers.ClassParameter.from(float.class, -1f),
                        ReflectionHelpers.ClassParameter.from(float.class, 1f),
                        ReflectionHelpers.ClassParameter.from(float.class, 0f),
                        ReflectionHelpers.ClassParameter.from(float.class, 0f),
                        ReflectionHelpers.ClassParameter.from(float.class, 0f));
            }
        }
        org.robolectric.Shadows.shadowOf((InputManager) activity.getSystemService(Context.INPUT_SERVICE))
                .addInputDevice(device);
    }

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

    @Test public void launchDoesNotWaitWithoutAWaveformReadyDualSense() throws Exception {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            Connection connection = new Connection(73);
            ControllerHandler handler = handler(activity.get(), connection);
            ExecutorService worker = Executors.newSingleThreadExecutor();
            try {
                // No USB DualSense is attached: nothing can become ready, so launch goes ahead at once.
                assertEquals("", worker.submit(handler::prepareControllersForLaunch).get(1, TimeUnit.SECONDS));
                // A handed-over DualSense whose waveform output could not start will not gain it either.
                usbManager(activity.get()).addOrUpdateUsbDevice(UsbPads.dualSense("/dev/bus/usb/002/001"), true);
                handler.deviceAdded(new Pad((short) MoonBridge.LI_CCAP_RUMBLE));
                assertFalse(worker.submit(handler::prepareControllersForLaunch).get(1, TimeUnit.SECONDS).isEmpty());
                assertEquals(1, connection.arrivals);
            } finally {
                worker.shutdownNow();
                handler.destroy();
            }
        }
    }

    @Test public void anAttachedDualSenseNotYetHandedOverMakesLaunchWait() throws Exception {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            usbManager(activity.get()).addOrUpdateUsbDevice(UsbPads.dualSense("/dev/bus/usb/002/002"), true);
            Connection connection = new Connection(73);
            ControllerHandler handler = handler(activity.get(), connection);
            ExecutorService worker = Executors.newSingleThreadExecutor();
            try {
                Future<String> launch = worker.submit(handler::prepareControllersForLaunch);
                try {
                    launch.get(300, TimeUnit.MILLISECONDS);
                    fail("Launch went ahead while the USB service was still starting the attached DualSense");
                } catch (TimeoutException expected) {
                }
                // The USB service hands the started pad over with its waveform output ready.
                handler.deviceAdded(new Pad());
                assertFalse(launch.get(2, TimeUnit.SECONDS).isEmpty());
            } finally {
                worker.shutdownNow();
                handler.destroy();
            }
        }
    }

    @Test public void aDualSenseAtTheUsbPermissionPromptDoesNotHoldTheLaunch() throws Exception {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            // Attached, but the person has not answered Android's USB permission prompt yet.
            usbManager(activity.get()).addOrUpdateUsbDevice(UsbPads.dualSense("/dev/bus/usb/002/003"), false);
            ControllerHandler handler = handler(activity.get(), new Connection(73));
            ExecutorService worker = Executors.newSingleThreadExecutor();
            try {
                assertEquals("", worker.submit(handler::prepareControllersForLaunch).get(1, TimeUnit.SECONDS));
            } finally {
                worker.shutdownNow();
                handler.destroy();
            }
        }
    }

    // Android 13 registers the USB service's unexported receiver without a manifest permission.
    @Config(sdk = 33)
    @Test public void aDualSenseWhoseDriverFailedToStartDoesNotHoldTheLaunch() throws Exception {
        // The test runs the main looper; the SHIELD accessory service is not on this device.
        org.robolectric.Shadows.shadowOf(org.robolectric.RuntimeEnvironment.getApplication()).declareComponentUnbindable(
                new android.content.ComponentName("com.nvidia.blakepairing", "com.nvidia.blakepairing.AccessoryService"));
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            android.preference.PreferenceManager.getDefaultSharedPreferences(activity.get()).edit()
                    .putBoolean("checkbox_usb_dualsense", true).commit();
            UsbPads.Connection.reset();
            // Android's own driver keeps the HID interface: the USB service cannot claim it.
            UsbPads.Connection.failClaims();
            usbManager(activity.get()).addOrUpdateUsbDevice(UsbPads.dualSense("/dev/bus/usb/002/004"), true);
            ControllerHandler handler = handler(activity.get(), new Connection(73));
            ServiceController<UsbDriverService> service = Robolectric.buildService(UsbDriverService.class).create();
            ExecutorService worker = Executors.newSingleThreadExecutor();
            try {
                UsbDriverService.UsbDriverBinder binder = (UsbDriverService.UsbDriverBinder) service.get().onBind(new Intent());
                binder.setListener(handler);
                binder.start();
                Future<String> launch = worker.submit(handler::prepareControllersForLaunch);
                long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(2);
                while (!launch.isDone() && System.nanoTime() < deadline) {
                    // The service reports the failed start on the main thread.
                    ShadowLooper.idleMainLooper();
                    Thread.sleep(10);
                }
                assertTrue("Launch kept waiting for a DualSense whose driver failed", launch.isDone());
                assertEquals("", launch.get());
                assertEquals(1, UsbPads.Connection.closes.get());
            } finally {
                worker.shutdownNow();
                service.destroy();
                handler.destroy();
            }
        }
    }

    private static ShadowUsbManager usbManager(Activity activity) {
        return org.robolectric.Shadows.shadowOf((UsbManager) activity.getSystemService(Context.USB_SERVICE));
    }

    @Test public void inputWaitsWhileLaunchPreparationAnnouncesAPad() throws Exception {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            CountDownLatch announcing = new CountDownLatch(1), release = new CountDownLatch(1);
            AtomicInteger inProgress = new AtomicInteger();
            AtomicBoolean overlapped = new AtomicBoolean();
            List<Byte> slots = Collections.synchronizedList(new ArrayList<>());
            Connection connection = new Connection(73) {
                @Override public int sendControllerArrivalEvent(byte slot, short mask, byte type, int buttons, short caps) {
                    if (inProgress.incrementAndGet() > 1) overlapped.set(true);
                    slots.add(slot);
                    if ((caps & MoonBridge.LI_CCAP_HAPTICS_PCM) != 0) {
                        // The connection worker is announcing the waveform pad before launch.
                        announcing.countDown();
                        try { release.await(2, TimeUnit.SECONDS); } catch (InterruptedException e) { Thread.currentThread().interrupt(); }
                    }
                    inProgress.decrementAndGet();
                    return super.sendControllerArrivalEvent(slot, mask, type, buttons, caps);
                }
            };
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.usbDualSenseHaptics = true;
            prefs.multiController = true;
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            handler.deviceAdded(new Pad());
            handler.deviceAdded(new Pad(43, MoonBridge.LI_CCAP_RUMBLE));
            ExecutorService threads = Executors.newFixedThreadPool(2);
            try {
                Future<String> launch = threads.submit(handler::prepareControllersForLaunch);
                assertTrue(announcing.await(2, TimeUnit.SECONDS));
                // The second pad's first input arrives meanwhile and needs a slot of its own.
                Future<?> input = threads.submit(() -> handler.reportControllerState(43, ControllerPacket.A_FLAG,
                        0, 0, 0, 0, 0, 0));
                try { input.get(300, TimeUnit.MILLISECONDS); } catch (TimeoutException expected) { }
                assertFalse("Slot assignment must not run on two threads at once", overlapped.get());
                release.countDown();
                assertFalse(launch.get(2, TimeUnit.SECONDS).isEmpty());
                input.get(2, TimeUnit.SECONDS);
                assertEquals(Arrays.asList((byte) 0, (byte) 1), slots);
            } finally {
                release.countDown();
                threads.shutdownNow();
                handler.destroy();
            }
        }
    }

    @Test public void inputDuringAnotherPadsSlotAssignmentSeesTheFinishedMask() throws Exception {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            CountDownLatch announcing = new CountDownLatch(1), release = new CountDownLatch(1);
            AtomicBoolean assigning = new AtomicBoolean();
            List<String> inputsDuringAssignment = Collections.synchronizedList(new ArrayList<>());
            Connection connection = new Connection(73) {
                @Override public int sendControllerArrivalEvent(byte slot, short mask, byte type, int buttons, short caps) {
                    if ((caps & MoonBridge.LI_CCAP_HAPTICS_PCM) != 0) {
                        // Launch preparation is still inside the waveform pad's slot assignment.
                        assigning.set(true);
                        announcing.countDown();
                        try { release.await(2, TimeUnit.SECONDS); } catch (InterruptedException e) { Thread.currentThread().interrupt(); }
                        assigning.set(false);
                    }
                    return super.sendControllerArrivalEvent(slot, mask, type, buttons, caps);
                }
                @Override public void sendControllerInput(short slot, short mask, int buttons, byte leftTrigger,
                                                         byte rightTrigger, short leftX, short leftY, short rightX, short rightY) {
                    if (assigning.get()) inputsDuringAssignment.add("slot " + slot + ", mask " + mask);
                    super.sendControllerInput(slot, mask, buttons, leftTrigger, rightTrigger, leftX, leftY, rightX, rightY);
                }
            };
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.usbDualSenseHaptics = true;
            prefs.multiController = true;
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            handler.deviceAdded(new Pad(43, MoonBridge.LI_CCAP_RUMBLE));
            handler.reportControllerState(43, ControllerPacket.A_FLAG, 0, 0, 0, 0, 0, 0);
            assertEquals(1, connection.inputs);
            handler.deviceAdded(new Pad());
            ExecutorService threads = Executors.newFixedThreadPool(2);
            try {
                Future<String> launch = threads.submit(handler::prepareControllersForLaunch);
                assertTrue(announcing.await(2, TimeUnit.SECONDS));
                // The pad that already has slot 0 keeps sending input on the main thread meanwhile.
                Future<?> input = threads.submit(() -> handler.reportControllerState(43, 0, 0, 0, 0, 0, 0, 0));
                try { input.get(300, TimeUnit.MILLISECONDS); } catch (TimeoutException expected) { }
                assertEquals("The active-pad mask must not be read mid-assignment", List.of(), inputsDuringAssignment);
                release.countDown();
                assertFalse(launch.get(2, TimeUnit.SECONDS).isEmpty());
                input.get(2, TimeUnit.SECONDS);
                assertEquals(2, connection.inputs);
            } finally {
                release.countDown();
                threads.shutdownNow();
                handler.destroy();
            }
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
                handler.resendControllerArrivals();
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
                assertFalse("No DualSense audio yet: the renderer skips its volume lookups",
                        handler.headphoneAudioSink().active());
                context.sendControllerArrival();
                assertTrue(handler.headphoneAudioSink().active());
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
                assertFalse(handler.headphoneAudioSink().active());
                assertEquals(1, connection.arrivals);
                assertEquals(1, pad.audioBindings);
                assertEquals(1, pad.audioPackets);
                assertEquals(1, pad.headphonePackets);
                assertEquals(1, pad.waveforms);
            } finally { context.destroy(); handler.destroy(); }
        }
    }

    @Test public void theOnScreenTouchpadButtonAsksTheHostForAPlayStationPad() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.multiController = true;
            prefs.onscreenController = true;
            // Without the Touchpad button the on-screen pad stays quiet, as before: the host's default pad.
            Connection quiet = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), quiet, null, prefs);
            try {
                assertEquals("", handler.prepareControllersForLaunch());
                handler.resendControllerArrivals();
                assertEquals(0, quiet.arrivals);
            } finally { handler.destroy(); }

            prefs.showTouchpadButton = true;
            Connection connection = new Connection(73);
            handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                assertEquals("0:" + MoonBridge.LI_CTYPE_PS + ":0:" + ControllerHandler.ONSCREEN_PAD_BUTTONS,
                        handler.prepareControllersForLaunch());
                handler.resendControllerArrivals();
                assertEquals(1, connection.arrivals);
                assertEquals(0, connection.arrivalSlot);
                assertEquals(MoonBridge.LI_CTYPE_PS, connection.arrivalType);
                assertTrue((connection.arrivalButtons & ControllerPacket.TOUCHPAD_FLAG) != 0);
            } finally { handler.destroy(); }
        }
    }

    @Test public void aPadInPlayerOnesSlotKeepsItsTypeWithTheTouchpadButton() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, true, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.multiController = true;
            prefs.onscreenController = true;
            prefs.showTouchpadButton = true;
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                String[] prepared = handler.prepareControllersForLaunch().split(",");
                assertEquals(1, prepared.length);
                assertTrue(prepared[0].startsWith("0:1:"));
                int before = connection.arrivals;
                handler.resendControllerArrivals();
                assertEquals(before + 1, connection.arrivals);
                assertTrue(connection.arrivalType != MoonBridge.LI_CTYPE_PS);
            } finally { handler.destroy(); }
        }
    }

    @Test public void builtInControlsCarryTheTouchpadButtonAsAPlayStationPad() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 8, false, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.multiController = true;
            prefs.onscreenController = true;
            // Without the Touchpad button the handheld's controls keep their own type.
            Connection quiet = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), quiet, null, prefs);
            try {
                assertTrue(handler.prepareControllersForLaunch().startsWith("0:1:"));
                assertTrue(quiet.arrivalType != MoonBridge.LI_CTYPE_PS);
                assertEquals(0, quiet.arrivalButtons & ControllerPacket.TOUCHPAD_FLAG);
            } finally { handler.destroy(); }

            prefs.showTouchpadButton = true;
            Connection connection = new Connection(73);
            handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                String prepared = handler.prepareControllersForLaunch();
                assertTrue(prepared, prepared.startsWith("0:" + MoonBridge.LI_CTYPE_PS + ":"));
                assertTrue((Integer.parseUnsignedInt(prepared.split(":")[3]) & ControllerPacket.TOUCHPAD_FLAG) != 0);
                handler.resendControllerArrivals();
                assertEquals(0, connection.arrivalSlot);
                assertEquals(MoonBridge.LI_CTYPE_PS, connection.arrivalType);
                assertTrue((connection.arrivalButtons & ControllerPacket.TOUCHPAD_FLAG) != 0);
                assertEquals("One pad for player 1, not a second on-screen one", 1, connection.typeBySlot.size());
            } finally { handler.destroy(); }
        }
    }

    @Test public void playerOnesTypeStaysForTheSessionWhenTheOnScreenControlsToggle() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 8, false, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.multiController = true;
            // Off at launch, turned on from the stream menu later to press Touchpad.
            prefs.onscreenController = false;
            prefs.showTouchpadButton = true;
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                assertTrue(handler.prepareControllersForLaunch().startsWith("0:" + MoonBridge.LI_CTYPE_PS + ":"));
                for (boolean shown : new boolean[] {true, false}) {
                    prefs.onscreenController = shown;
                    handler.resendControllerArrivals();
                    assertEquals(MoonBridge.LI_CTYPE_PS, connection.arrivalType);
                    assertTrue((connection.arrivalButtons & ControllerPacket.TOUCHPAD_FLAG) != 0);
                }
            } finally { handler.destroy(); }
        }
    }

    @Test public void aHandheldsOwnPadListedAsExternalIsToldApartFromARealXboxPad() {
        // Retroid Pocket 6: "Xbox Wireless Controller", 0x2022:0x3002, external, on an internal bus.
        assertTrue(ControllerHandler.looksLikeHandheldControls("Xbox Wireless Controller", 0x2022, true, false));
        assertFalse("A real Xbox pad", ControllerHandler.looksLikeHandheldControls("Xbox Wireless Controller", 0x045e, true, false));
        assertFalse("Beside another pad", ControllerHandler.looksLikeHandheldControls("Xbox Wireless Controller", 0x2022, false, false));
        assertFalse("Attached by USB", ControllerHandler.looksLikeHandheldControls("Xbox Wireless Controller", 0x2022, true, true));
        assertFalse("Another kind of pad", ControllerHandler.looksLikeHandheldControls("8BitDo Pro 2", 0x2dc8, true, false));
    }

    @Test public void aHandheldsOwnPadCarriesTheTouchpadButton() {
        Connection connection = arrivalOfXboxNamedPad(0x2022);
        assertEquals(MoonBridge.LI_CTYPE_PS, connection.arrivalType);
        assertTrue((connection.arrivalButtons & ControllerPacket.TOUCHPAD_FLAG) != 0);
    }

    @Test public void aRealXboxPadKeepsItsTypeBesideTheTouchpadButton() {
        Connection connection = arrivalOfXboxNamedPad(0x045e);
        assertEquals(MoonBridge.LI_CTYPE_XBOX, connection.arrivalType);
        assertEquals(0, connection.arrivalButtons & ControllerPacket.TOUCHPAD_FLAG);
    }

    /** The only gamepad, listed as external and named like a Retroid Pocket's, with the Touchpad button on. */
    private static Connection arrivalOfXboxNamedPad(int vendor) {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 7, true, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD,
                    "Xbox Wireless Controller", vendor, 0x3002);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.multiController = true;
            prefs.onscreenController = true;
            prefs.showTouchpadButton = true;
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                String prepared = handler.prepareControllersForLaunch();
                handler.resendControllerArrivals();
                assertTrue(prepared, prepared.startsWith("0:" + connection.arrivalType + ":"));
                assertEquals(0, connection.arrivalSlot);
                return connection;
            } finally { handler.destroy(); }
        }
    }

    @Test public void anExternalPadKeepsItsTypeBesideTheTouchpadButton() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, true, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            addInputDevice(activity.get(), 4, true, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            addInputDevice(activity.get(), 8, false, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.multiController = true;
            prefs.onscreenController = true;
            prefs.showTouchpadButton = true;
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                String[] prepared = handler.prepareControllersForLaunch().split(",");
                assertEquals(2, prepared.length);
                assertTrue(prepared[0].startsWith("0:1:"));
                assertTrue(prepared[1].startsWith("1:1:"));
                handler.resendControllerArrivals();
                for (byte type : connection.typeBySlot.values()) assertTrue(type != MoonBridge.LI_CTYPE_PS);
            } finally { handler.destroy(); }
        }
    }

    private static ControllerHandler handler(Activity activity, Connection connection) {
        PreferenceConfiguration prefs = new PreferenceConfiguration(); prefs.usbDualSenseHaptics = true;
        return new ControllerHandler(activity, connection, null, prefs);
    }
    private static class Connection extends NvConnection {
        int arrivals, inputs;
        short activeMask;
        byte arrivalType;
        short arrivalCaps;
        byte arrivalSlot;
        int arrivalButtons;
        final java.util.Map<Byte, Byte> typeBySlot = new java.util.HashMap<>();
        /** Each arrival as "arrival slot" and each input as "input slot buttons", in order. */
        final List<String> events = new ArrayList<>();
        Connection(int appId) {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().setApp(new NvApp("Test", appId, false)).build(), null, null);
        }
        @Override public int sendControllerArrivalEvent(byte slot, short mask, byte type, int buttons, short caps) {
            arrivals++; arrivalType = type; arrivalCaps = caps; arrivalSlot = slot; arrivalButtons = buttons;
            typeBySlot.put(slot, type); events.add("arrival " + slot); return 0;
        }
        @Override public void sendControllerInput(short slot, short mask, int buttons, byte leftTrigger,
                                                 byte rightTrigger, short leftX, short leftY, short rightX, short rightY) {
            inputs++; activeMask = mask; events.add("input " + slot + " " + buttons);
        }
    }
    private static class Pad extends AbstractController {
        int audioBindings, audioPackets, headphonePackets, waveforms;
        Pad() { this(MoonBridge.LI_CCAP_HAPTICS_PCM); }
        Pad(short caps) { this(42, caps); }
        Pad(int id, short caps) { super(id, null, 0x054c, 0x0ce6); capabilities = caps; }
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

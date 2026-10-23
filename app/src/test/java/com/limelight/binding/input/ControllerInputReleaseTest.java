package com.limelight.binding.input;

import android.app.Activity;
import android.app.Application;
import android.view.KeyEvent;

import com.limelight.Game;
import com.limelight.binding.input.capture.InputCaptureProvider;
import com.limelight.binding.input.driver.AbstractController;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.nvstream.input.MouseButtonPacket;
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
import java.util.Map;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class ControllerInputReleaseTest {
    @Test
    public void androidControllerMenuComboWaitsForReleaseWithoutQuitting() {
        MenuGame activity = Robolectric.buildActivity(MenuGame.class).get();
        ControllerHandler handler = new ControllerHandler(activity, new RecordingConnection(), null, new PreferenceConfiguration());
        ControllerHandler.InputDeviceContext context = ReflectionHelpers.getField(handler, "defaultContext");
        context.assignedControllerNumber = true;
        context.hasSelect = context.hasMode = true;
        int[] keys = {KeyEvent.KEYCODE_BUTTON_L1, KeyEvent.KEYCODE_BUTTON_R1,
                KeyEvent.KEYCODE_BUTTON_SELECT, KeyEvent.KEYCODE_BUTTON_START};
        try {
            for (int key : keys) {
                assertTrue(handler.handleButtonDown(new KeyEvent(1000, 1000, KeyEvent.ACTION_DOWN,
                        key, 0, 0, 0, 0)));
            }
            assertTrue(context.pendingExit);
            for (int i = 0; i < keys.length; i++) {
                assertEquals(0, activity.menuCount);
                assertTrue(handler.handleButtonUp(new KeyEvent(1000, 1100, KeyEvent.ACTION_UP,
                        keys[i], 0, 0, 0, 0)));
            }
            assertEquals(1, activity.menuCount);
            assertFalse(context.pendingExit);
            assertFalse(activity.isFinishing());
            handler.handleButtonUp(new KeyEvent(1000, 1100, KeyEvent.ACTION_UP,
                    KeyEvent.KEYCODE_BUTTON_START, 0, 0, 0, 0));
            assertEquals(1, activity.menuCount);
        } finally {
            handler.destroy();
        }
    }

    public static class MenuGame extends Game {
        int menuCount;
        final List<Integer> steered = new ArrayList<>();
        @Override public void showStreamMenu() { menuCount++; }
        @Override public void steerStreamMenu(int keyCode) { steered.add(keyCode); }
    }

    private static ControllerHandler.UsbDeviceContext usbPad(ControllerHandler handler) {
        Map<Integer, ControllerHandler.UsbDeviceContext> usb = ReflectionHelpers.getField(handler, "usbDeviceContexts");
        ControllerHandler.UsbDeviceContext context = handler.new UsbDeviceContext();
        context.assignedControllerNumber = true;
        context.buttonMap = new ControllerButtonMap();
        context.device = new UsbController();
        usb.put(3, context);
        return context;
    }

    private static void usb(ControllerHandler handler, int buttons) {
        handler.reportControllerState(3, buttons, 0, 0, 0, 0, 0, 0);
    }

    @Test
    public void usbDriverMenuComboOpensTheMenuOnceEveryButtonIsUp() {
        MenuGame activity = Robolectric.buildActivity(MenuGame.class).get();
        RecordingConnection connection = new RecordingConnection();
        ControllerHandler handler = new ControllerHandler(activity, connection, null, new PreferenceConfiguration());
        ControllerHandler.UsbDeviceContext pad = usbPad(handler);
        int combo = ControllerPacket.BACK_FLAG | ControllerPacket.PLAY_FLAG | ControllerPacket.LB_FLAG | ControllerPacket.RB_FLAG;
        try {
            usb(handler, ControllerPacket.LB_FLAG | ControllerPacket.RB_FLAG);
            usb(handler, combo);
            assertTrue(pad.menuComboPending);
            usb(handler, ControllerPacket.PLAY_FLAG | ControllerPacket.LB_FLAG);
            assertEquals("Not while a button of the combination is still held", 0, activity.menuCount);
            usb(handler, 0);
            assertEquals(1, activity.menuCount);
            assertFalse(activity.isFinishing());
            // Releasing again, or Start with the bumpers alone, opens nothing.
            usb(handler, 0);
            usb(handler, ControllerPacket.PLAY_FLAG | ControllerPacket.LB_FLAG | ControllerPacket.RB_FLAG);
            usb(handler, 0);
            assertEquals(1, activity.menuCount);
        } finally {
            handler.destroy();
        }
    }

    @Test
    public void usbDriverPadSteersTheOpenMenuAndItsChoiceDoesNotReachTheGame() {
        MenuGame activity = Robolectric.buildActivity(MenuGame.class).get();
        RecordingConnection connection = new RecordingConnection();
        ControllerHandler handler = new ControllerHandler(activity, connection, null, new PreferenceConfiguration());
        usbPad(handler);
        try {
            usb(handler, ControllerPacket.X_FLAG);
            handler.setInputEnabled(false);
            int sent = connection.states.size();
            // X was held when the menu opened: it is no new press.
            usb(handler, ControllerPacket.X_FLAG | ControllerPacket.DOWN_FLAG);
            usb(handler, ControllerPacket.X_FLAG);
            usb(handler, ControllerPacket.A_FLAG);
            usb(handler, ControllerPacket.A_FLAG | ControllerPacket.B_FLAG);
            assertEquals(java.util.Arrays.asList(KeyEvent.KEYCODE_DPAD_DOWN, KeyEvent.KEYCODE_DPAD_CENTER,
                    KeyEvent.KEYCODE_BUTTON_B), activity.steered);
            assertEquals("Nothing reaches the host behind the menu", sent, connection.states.size());
            // The menu closed with A still held: the game sees A only after a new press.
            handler.setInputEnabled(true);
            usb(handler, ControllerPacket.A_FLAG);
            assertEquals(0, connection.states.get(connection.states.size() - 1)[0] & ControllerPacket.A_FLAG);
            usb(handler, 0);
            usb(handler, ControllerPacket.A_FLAG);
            assertEquals(ControllerPacket.A_FLAG, connection.states.get(connection.states.size() - 1)[0] & ControllerPacket.A_FLAG);
        } finally {
            handler.destroy();
        }
    }

    @Test
    public void usbDriverPadSteersThePcKeyboardAndGivesTheGameOnlyNewPressesAfterwards() {
        MenuGame activity = Robolectric.buildActivity(MenuGame.class).get();
        RecordingConnection connection = new RecordingConnection();
        ControllerHandler handler = new ControllerHandler(activity, connection, null, new PreferenceConfiguration());
        usbPad(handler);
        List<String> events = new ArrayList<>();
        try {
            usb(handler, ControllerPacket.X_FLAG);
            handler.setPadCapture(new ControllerHandler.PadCapture() {
                @Override public void button(int flag, boolean down) { events.add(flag + (down ? "+" : "-")); }
                @Override public void stick(float x, float y) { events.add("stick"); }
            });
            // X was held before the keyboard opened, and A arrives on the first report it sees: neither is
            // a press for the keyboard; X is only seen being let go. The game's last state is cleared from
            // this thread with a neutral packet.
            usb(handler, ControllerPacket.X_FLAG | ControllerPacket.A_FLAG);
            int[] neutral = connection.states.get(connection.states.size() - 1);
            assertEquals("The capture starts neutral", 0, neutral[0]);
            int sent = connection.states.size();
            usb(handler, ControllerPacket.A_FLAG);
            usb(handler, 0);
            usb(handler, ControllerPacket.A_FLAG);
            assertEquals(java.util.Arrays.asList(ControllerPacket.X_FLAG + "-", ControllerPacket.A_FLAG + "-",
                    ControllerPacket.A_FLAG + "+"), events);
            assertEquals("Nothing reaches the host while the keyboard is open", sent, connection.states.size());
            // Closed with A held: the game sees A only after a new press.
            handler.setPadCapture(null);
            usb(handler, ControllerPacket.A_FLAG);
            assertEquals(0, connection.states.get(connection.states.size() - 1)[0] & ControllerPacket.A_FLAG);
            usb(handler, 0);
            usb(handler, ControllerPacket.A_FLAG);
            assertEquals(ControllerPacket.A_FLAG, connection.states.get(connection.states.size() - 1)[0] & ControllerPacket.A_FLAG);
        } finally {
            handler.destroy();
        }
    }

    @Test
    public void usbDriverStickReachesThePcKeyboardOnlyWhenItsDirectionChanges() {
        MenuGame activity = Robolectric.buildActivity(MenuGame.class).get();
        RecordingConnection connection = new RecordingConnection();
        ControllerHandler handler = new ControllerHandler(activity, connection, null, new PreferenceConfiguration());
        usbPad(handler);
        List<String> sticks = new ArrayList<>();
        try {
            handler.setPadCapture(new ControllerHandler.PadCapture() {
                @Override public void button(int flag, boolean down) { }
                @Override public void stick(float x, float y) { sticks.add(x + "," + y); }
            });
            handler.reportControllerState(3, 0, 0f, 0f, 0f, 0f, 0f, 0f);
            handler.reportControllerState(3, 0, 0.1f, 0.2f, 0f, 0f, 0f, 0f);
            assertTrue("Centred reports are not news", sticks.isEmpty());
            handler.reportControllerState(3, 0, 0.7f, 0.1f, 0f, 0f, 0f, 0f);
            handler.reportControllerState(3, 0, 0.8f, 0.2f, 0f, 0f, 0f, 0f);
            handler.reportControllerState(3, 0, 0.5f, 0.2f, 0f, 0f, 0f, 0f);
            assertEquals("Held right, and still right under the engage threshold", 1, sticks.size());
            handler.reportControllerState(3, 0, 0.3f, 0.1f, 0f, 0f, 0f, 0f);
            handler.reportControllerState(3, 0, 0.1f, 0f, 0f, 0f, 0f, 0f);
            assertEquals("Back to centre", 2, sticks.size());
        } finally {
            handler.destroy();
        }
    }

    @Test
    public void unpluggingAUsbPadLetsGoOfTheKeyboardButtonsItHeld() {
        MenuGame activity = Robolectric.buildActivity(MenuGame.class).get();
        RecordingConnection connection = new RecordingConnection();
        ControllerHandler handler = new ControllerHandler(activity, connection, null, new PreferenceConfiguration());
        ControllerHandler.UsbDeviceContext pad = usbPad(handler);
        List<String> events = new ArrayList<>();
        try {
            handler.setPadCapture(new ControllerHandler.PadCapture() {
                @Override public void button(int flag, boolean down) { events.add(flag + (down ? "+" : "-")); }
                @Override public void stick(float x, float y) { events.add("stick " + x + "," + y); }
            });
            usb(handler, 0);
            handler.reportControllerState(3, ControllerPacket.X_FLAG, 0.9f, 0f, 0f, 0f, 0f, 0f);
            assertEquals(java.util.Arrays.asList(ControllerPacket.X_FLAG + "+", "stick 0.9,0.0"), events);
            events.clear();
            handler.deviceRemoved((AbstractController) pad.device);
            // Nothing reports the release of X or the stick any more: the keyboard hears it now.
            assertEquals(java.util.Arrays.asList(ControllerPacket.X_FLAG + "-", "stick 0.0,0.0"), events);
        } finally {
            handler.destroy();
        }
    }

    @Test
    public void unpluggingAUsbPadWithoutACaptureReportsNothing() {
        MenuGame activity = Robolectric.buildActivity(MenuGame.class).get();
        ControllerHandler handler = new ControllerHandler(activity, new RecordingConnection(), null, new PreferenceConfiguration());
        ControllerHandler.UsbDeviceContext pad = usbPad(handler);
        try {
            usb(handler, ControllerPacket.X_FLAG);
            handler.deviceRemoved((AbstractController) pad.device);
        } finally {
            handler.destroy();
        }
    }

    @Test
    public void aHeldUsbStickIsReportedAgainAfterTheMenuOrAResync() {
        MenuGame activity = Robolectric.buildActivity(MenuGame.class).get();
        RecordingConnection connection = new RecordingConnection();
        ControllerHandler handler = new ControllerHandler(activity, connection, null, new PreferenceConfiguration());
        usbPad(handler);
        List<String> sticks = new ArrayList<>();
        try {
            handler.setPadCapture(new ControllerHandler.PadCapture() {
                @Override public void button(int flag, boolean down) { }
                @Override public void stick(float x, float y) { sticks.add(x + "," + y); }
            });
            handler.reportControllerState(3, 0, 0.9f, 0f, 0f, 0f, 0f, 0f);
            handler.reportControllerState(3, 0, 0.9f, 0f, 0f, 0f, 0f, 0f);
            assertEquals(1, sticks.size());
            // A layout switch released everything on the keyboard side while the stick stays pushed.
            handler.resyncPadCapture();
            handler.reportControllerState(3, 0, 0.9f, 0f, 0f, 0f, 0f, 0f);
            handler.reportControllerState(3, 0, 0.9f, 0f, 0f, 0f, 0f, 0f);
            assertEquals(2, sticks.size());
            // The stream menu steers the pad for a while: the keyboard let go of the stick then too.
            handler.setInputEnabled(false);
            usb(handler, 0);
            handler.setInputEnabled(true);
            handler.reportControllerState(3, 0, 0.9f, 0f, 0f, 0f, 0f, 0f);
            assertEquals(3, sticks.size());
            // Handing the pad back to the game and taking it again starts afresh as well.
            handler.setPadCapture(null);
            usb(handler, 0);
            handler.setPadCapture(new ControllerHandler.PadCapture() {
                @Override public void button(int flag, boolean down) { }
                @Override public void stick(float x, float y) { sticks.add(x + "," + y); }
            });
            handler.reportControllerState(3, 0, 0.9f, 0f, 0f, 0f, 0f, 0f);
            assertEquals(4, sticks.size());
        } finally {
            handler.destroy();
        }
    }

    @Test
    public void releasingInputClearsEveryContextAndDoesNotFuseStaleAxes() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            RecordingConnection connection = new RecordingConnection();
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, new PreferenceConfiguration());
            Map<Integer, ControllerHandler.InputDeviceContext> devices = ReflectionHelpers.getField(handler, "inputDeviceContexts");
            Map<Integer, ControllerHandler.UsbDeviceContext> usb = ReflectionHelpers.getField(handler, "usbDeviceContexts");
            ControllerHandler.InputDeviceContext first = handler.new InputDeviceContext();
            ControllerHandler.InputDeviceContext second = handler.new InputDeviceContext();
            ControllerHandler.UsbDeviceContext directUsb = handler.new UsbDeviceContext();
            directUsb.device = new UsbController();
            ControllerHandler.InputDeviceContext defaults = ReflectionHelpers.getField(handler, "defaultContext");
            // The host has seen player 1: the on-screen pad pressed a button there.
            handler.reportOscState(ControllerPacket.A_FLAG, (short) 0, (short) 0, (short) 0, (short) 0,
                    (byte) 0, (byte) 0);
            assertEquals(1, connection.states.size());
            connection.states.clear();
            devices.put(1, first);
            devices.put(2, second);
            usb.put(3, directUsb);
            first.pressedButtons.put(304, KeyEvent.KEYCODE_BUTTON_A);
            first.emulatingButtonFlags = 1;
            first.pendingExit = true;
            first.mouseEmulationLastInputMap = ControllerPacket.A_FLAG | ControllerPacket.B_FLAG;
            List<ControllerHandler.GenericControllerContext> contexts = List.of(first, second, directUsb, defaults);
            for (ControllerHandler.GenericControllerContext context : contexts) {
                context.assignedControllerNumber = true;
                context.controllerNumber = 0;
                context.inputMap = ControllerPacket.A_FLAG | ControllerPacket.UP_FLAG;
                context.leftTrigger = context.rightTrigger = (byte) 255;
                context.leftStickX = context.rightStickX = 10000;
                context.leftStickY = context.rightStickY = -10000;
            }
            try {
                handler.releaseInput();
                assertEquals(4, connection.states.size());
                for (int[] state : connection.states) assertArrayEquals(new int[7], state);
                assertEquals(List.of(MouseButtonPacket.BUTTON_LEFT, MouseButtonPacket.BUTTON_RIGHT), connection.mouseUps);
                for (ControllerHandler.GenericControllerContext context : contexts) {
                    assertEquals(0, context.inputMap);
                    assertEquals(0, context.leftTrigger);
                    assertEquals(0, context.rightTrigger);
                    assertEquals(0, context.leftStickX);
                    assertEquals(0, context.leftStickY);
                    assertEquals(0, context.rightStickX);
                    assertEquals(0, context.rightStickY);
                }
                assertTrue(first.pressedButtons.isEmpty());
                assertEquals(0, first.emulatingButtonFlags);
                assertFalse(first.pendingExit);
                assertTrue(ControllerHandler.updateMappedButton(first.pressedButtons, 304, KeyEvent.KEYCODE_BUTTON_A, true));
            } finally {
                usb.clear();
                handler.destroy();
            }
        }
    }

    @Test
    public void menuFocusBeforeAnyControllerInputDoesNotAnnounceAPadToTheHost() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            RecordingConnection connection = new RecordingConnection();
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, new PreferenceConfiguration());
            Map<Integer, ControllerHandler.InputDeviceContext> devices = ReflectionHelpers.getField(handler, "inputDeviceContexts");
            // A Bluetooth pad that has a slot but has not pressed anything yet: its arrival
            // (type, touchpad, motion) is still to come with its first input.
            ControllerHandler.InputDeviceContext idlePad = handler.new InputDeviceContext();
            idlePad.assignedControllerNumber = true;
            idlePad.controllerNumber = 0;
            devices.put(7, idlePad);
            try {
                // Opening the stream menu and losing window focus both release input.
                handler.setInputEnabled(false);
                handler.setInputEnabled(true);
                handler.setInputEnabled(false);
                assertTrue("A neutral report would make the host create a default pad in slot 0",
                        connection.states.isEmpty());

                handler.setInputEnabled(true);
                handler.reportOscState(ControllerPacket.A_FLAG, (short) 0, (short) 0, (short) 0, (short) 0,
                        (byte) 0, (byte) 0);
                assertEquals(1, connection.states.size());
                handler.setInputEnabled(false);
                assertTrue("A slot that received input must still be released", connection.states.size() > 1);
                for (int[] state : connection.states.subList(1, connection.states.size())) {
                    assertArrayEquals(new int[7], state);
                }
            } finally {
                handler.destroy();
            }
        }
    }

    @Test
    public void usbMotionStopsWhileTheMenuHasFocus() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            RecordingConnection connection = new RecordingConnection();
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.gamepadMotionSensors = true;
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            Map<Integer, ControllerHandler.UsbDeviceContext> usb = ReflectionHelpers.getField(handler, "usbDeviceContexts");
            ControllerHandler.UsbDeviceContext context = handler.new UsbDeviceContext();
            context.assignedControllerNumber = true;
            context.buttonMap = new ControllerButtonMap();
            UsbController pad = new UsbController();
            pad.motion();
            context.device = pad;
            usb.put(3, context);
            try {
                handler.reportControllerMotion(3, MoonBridge.LI_MOTION_TYPE_GYRO, 10, 20, 30);
                assertEquals(1, connection.motions.size());
                handler.setInputEnabled(false);
                assertEquals("Opening the menu stops the turning camera", 2, connection.motions.size());
                assertArrayEquals(new float[] {MoonBridge.LI_MOTION_TYPE_GYRO, 0, 0, 0}, connection.motions.get(1), 0);
                handler.reportControllerMotion(3, MoonBridge.LI_MOTION_TYPE_GYRO, 10, 20, 30);
                handler.reportControllerMotion(3, MoonBridge.LI_MOTION_TYPE_ACCEL, 1, 2, 3);
                assertEquals("Motion must not reach the host behind the menu", 2, connection.motions.size());
                handler.setInputEnabled(true);
                handler.reportControllerMotion(3, MoonBridge.LI_MOTION_TYPE_GYRO, 10, 20, 30);
                assertEquals(3, connection.motions.size());
                assertArrayEquals(new float[] {MoonBridge.LI_MOTION_TYPE_GYRO, 10, 20, 30}, connection.motions.get(2), 0);
            } finally {
                usb.clear();
                handler.destroy();
            }
        }
    }

    @Test
    public void usbReportsCannotRestoreHeldInputWhileTheMenuHasFocus() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            RecordingConnection connection = new RecordingConnection();
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, new PreferenceConfiguration());
            Map<Integer, ControllerHandler.UsbDeviceContext> usb = ReflectionHelpers.getField(handler, "usbDeviceContexts");
            ControllerHandler.UsbDeviceContext context = handler.new UsbDeviceContext();
            context.assignedControllerNumber = true;
            context.buttonMap = new ControllerButtonMap();
            context.device = new UsbController();
            usb.put(3, context);
            try {
                handler.reportControllerTouch(3, MoonBridge.LI_TOUCH_EVENT_DOWN, 1, 0.5f, 0.5f, 1);
                handler.setInputEnabled(false);
                assertEquals(List.of(MoonBridge.LI_TOUCH_EVENT_DOWN, MoonBridge.LI_TOUCH_EVENT_CANCEL_ALL),
                        connection.touches);
                handler.reportControllerTouch(3, MoonBridge.LI_TOUCH_EVENT_UP, 1, 0.5f, 0.5f, 0);
                assertEquals(2, connection.touches.size());
                connection.states.clear();
                handler.reportControllerState(3, ControllerPacket.A_FLAG, 1, 1, 1, 1, 1, 1);
                assertTrue(connection.states.isEmpty());
                assertEquals(0, context.leftStickX);
                handler.setInputEnabled(true);
                // A was pressed in the menu, as an Android-mapped pad's press goes to the menu: the game
                // gets the sticks at once and A only once it is pressed again.
                handler.reportControllerState(3, ControllerPacket.A_FLAG, 1, 1, 1, 1, 1, 1);
                assertEquals(1, connection.states.size());
                assertEquals(0, connection.states.get(0)[0]);
                assertTrue(context.leftStickX != 0);
                handler.reportControllerState(3, 0, 1, 1, 1, 1, 1, 1);
                handler.reportControllerState(3, ControllerPacket.A_FLAG, 1, 1, 1, 1, 1, 1);
                assertEquals(ControllerPacket.A_FLAG, connection.states.get(connection.states.size() - 1)[0]);
                handler.reportControllerTouch(3, MoonBridge.LI_TOUCH_EVENT_DOWN, 2, 0.5f, 0.5f, 1);
                assertEquals(3, connection.touches.size());
            } finally {
                usb.clear();
                handler.destroy();
            }
        }
    }

    @Test
    public void cursorShortcutRestoresUsbInputAfterRegrabbing() {
        Game activity = Robolectric.buildActivity(FocusedGame.class).get();
        RecordingConnection connection = new RecordingConnection();
        ControllerHandler handler = new ControllerHandler(activity, connection, null, new PreferenceConfiguration());
        Map<Integer, ControllerHandler.UsbDeviceContext> usb = ReflectionHelpers.getField(handler, "usbDeviceContexts");
        ControllerHandler.UsbDeviceContext context = handler.new UsbDeviceContext();
        context.assignedControllerNumber = true;
        context.buttonMap = new ControllerButtonMap();
        context.device = new UsbController();
        usb.put(3, context);
        InputCaptureProvider capture = new InputCaptureProvider() { };
        ReflectionHelpers.setField(activity, "conn", connection);
        ReflectionHelpers.setField(activity, "controllerHandler", handler);
        ReflectionHelpers.setField(activity, "inputCaptureProvider", capture);
        ReflectionHelpers.setField(activity, "grabbedInput", false);
        try {
            handler.setInputEnabled(false);
            connection.states.clear();
            int[] keys = {KeyEvent.KEYCODE_CTRL_LEFT, KeyEvent.KEYCODE_ALT_LEFT, KeyEvent.KEYCODE_SHIFT_LEFT,
                    KeyEvent.KEYCODE_C};
            for (int key : keys) {
                ReflectionHelpers.callInstanceMethod(activity, "handleSpecialKeys",
                        ReflectionHelpers.ClassParameter.from(int.class, key),
                        ReflectionHelpers.ClassParameter.from(boolean.class, true));
            }
            for (int i = keys.length - 1; i >= 0; i--) {
                ReflectionHelpers.callInstanceMethod(activity, "handleSpecialKeys",
                        ReflectionHelpers.ClassParameter.from(int.class, keys[i]),
                        ReflectionHelpers.ClassParameter.from(boolean.class, false));
            }
            assertTrue(capture.isCapturingEnabled());
            handler.reportControllerState(3, ControllerPacket.A_FLAG, 1, 1, 1, 1, 1, 1);
            assertEquals(1, connection.states.size());
            assertEquals(ControllerPacket.A_FLAG, connection.states.get(0)[0]);
        } finally {
            usb.clear();
            handler.destroy();
        }
    }

    public static class FocusedGame extends Game {
        @Override public boolean hasWindowFocus() { return true; }
    }

    private static class UsbController extends AbstractController {
        UsbController() {
            super(3, null, 0x054c, 0x0ce6);
            capabilities = MoonBridge.LI_CCAP_TOUCHPAD;
        }

        void motion() {
            capabilities |= MoonBridge.LI_CCAP_ACCEL | MoonBridge.LI_CCAP_GYRO;
        }

        @Override public boolean start() { return true; }
        @Override public void stop() { }
        @Override public void rumble(short low, short high) { }
        @Override public void rumbleTriggers(short left, short right) { }
    }

    private static class RecordingConnection extends NvConnection {
        final List<int[]> states = new ArrayList<>();
        final List<Byte> mouseUps = new ArrayList<>();
        final List<Byte> touches = new ArrayList<>();
        final List<float[]> motions = new ArrayList<>();

        RecordingConnection() {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }

        @Override public void sendControllerInput(short number, short mask, int buttons, byte leftTrigger,
                                                   byte rightTrigger, short leftX, short leftY, short rightX, short rightY) {
            states.add(new int[] {buttons, leftTrigger, rightTrigger, leftX, leftY, rightX, rightY});
        }

        @Override public void sendMouseButtonUp(byte button) {
            mouseUps.add(button);
        }

        @Override public int sendControllerTouchEvent(byte number, byte event, int pointer,
                                                     float x, float y, float pressure) {
            touches.add(event);
            return 0;
        }

        @Override public int sendControllerMotionEvent(byte number, byte type, float x, float y, float z) {
            motions.add(new float[] {type, x, y, z});
            return 0;
        }
    }
}

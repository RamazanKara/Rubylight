package com.limelight;

import android.app.Application;
import android.view.InputDevice;
import android.view.KeyEvent;

import com.limelight.binding.input.ControllerHandler;
import com.limelight.binding.input.KeyboardTranslator;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.input.KeyboardPacket;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.Shadows;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowInputDevice;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = {28, 30}, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameRemoteControlTest {
    private InputDevice attached;

    public static class RemoteGame extends Game {
        int menus, backs;
        @Override public void showStreamMenu() { menus++; }
        @Override public void onBackPressed() { backs++; }
    }

    private InputDevice device(int sources, int keyboard) {
        InputDevice device = ShadowInputDevice.makeInputDeviceNamed("Fire TV Remote");
        ReflectionHelpers.setField(device, "mId", 12);
        ReflectionHelpers.setField(device, "mSources", sources);
        ReflectionHelpers.setField(device, "mKeyboardType", keyboard);
        attached = device;
        return device;
    }

    private KeyEvent event(int action, int key, int deviceId, int source) {
        KeyEvent event = new KeyEvent(100, 150, action, key, 0, 0, deviceId, 0, 0, source);
        Shadows.shadowOf(event).setDevice(attached != null && attached.getId() == deviceId ? attached : null);
        return event;
    }

    private static RemoteGame game(RecordingConnection connection) {
        RemoteGame game = Robolectric.buildActivity(RemoteGame.class).get();
        ReflectionHelpers.setField(game, "conn", connection);
        ReflectionHelpers.setField(game, "prefConfig", new PreferenceConfiguration());
        ReflectionHelpers.setField(game, "keyboardTranslator", new KeyboardTranslator());
        ReflectionHelpers.setField(game, "grabbedInput", true);
        return game;
    }

    @Test public void fireRemoteIsNotAGamepadEvenWhenItSendsKeyboardEvents() {
        InputDevice remote = device(InputDevice.SOURCE_DPAD | InputDevice.SOURCE_KEYBOARD,
                InputDevice.KEYBOARD_TYPE_NON_ALPHABETIC);
        assertFalse(ControllerHandler.isGameControllerDevice(remote));
        assertTrue(ControllerHandler.isRemoteControlEvent(
                event(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BACK, remote.getId(), InputDevice.SOURCE_KEYBOARD)));
    }

    @Test public void gamepadAndAlphabeticKeyboardAreNotRemotes() {
        InputDevice pad = device(InputDevice.SOURCE_DPAD | InputDevice.SOURCE_GAMEPAD,
                InputDevice.KEYBOARD_TYPE_NON_ALPHABETIC);
        assertTrue(ControllerHandler.isGameControllerDevice(pad));
        assertFalse(ControllerHandler.isRemoteControlEvent(
                event(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_MENU, pad.getId(), InputDevice.SOURCE_DPAD)));
        InputDevice keyboard = device(InputDevice.SOURCE_DPAD | InputDevice.SOURCE_KEYBOARD,
                InputDevice.KEYBOARD_TYPE_ALPHABETIC);
        assertFalse(ControllerHandler.isRemoteControlEvent(
                event(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BACK, keyboard.getId(), InputDevice.SOURCE_KEYBOARD)));
    }

    @Test public void backAndMenuStayLocalAndOnlyActOnUncancelledRelease() {
        InputDevice remote = device(InputDevice.SOURCE_DPAD, InputDevice.KEYBOARD_TYPE_NON_ALPHABETIC);
        RecordingConnection connection = new RecordingConnection();
        RemoteGame game = game(connection);
        for (int key : new int[] {KeyEvent.KEYCODE_BACK, KeyEvent.KEYCODE_MENU}) {
            KeyEvent down = event(KeyEvent.ACTION_DOWN, key, remote.getId(), InputDevice.SOURCE_DPAD);
            assertTrue(game.handleKeyDown(down));
            assertEquals(0, game.backs + game.menus);
            assertTrue(game.handleKeyUp(KeyEvent.changeFlags(KeyEvent.changeAction(down, KeyEvent.ACTION_UP),
                    KeyEvent.FLAG_CANCELED)));
            assertEquals(0, game.backs + game.menus);
        }
        assertTrue(game.handleKeyUp(event(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BACK, remote.getId(), InputDevice.SOURCE_DPAD)));
        assertTrue(game.handleKeyUp(event(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_MENU, remote.getId(), InputDevice.SOURCE_DPAD)));
        assertEquals(1, game.backs);
        assertEquals(1, game.menus);
        assertTrue(connection.keys.isEmpty());
    }

    @Test public void dpadAndSelectSendArrowKeysAndEnterWithMatchingReleases() {
        InputDevice remote = device(InputDevice.SOURCE_DPAD, InputDevice.KEYBOARD_TYPE_NON_ALPHABETIC);
        RecordingConnection connection = new RecordingConnection();
        RemoteGame game = game(connection);
        int[] buttons = {KeyEvent.KEYCODE_DPAD_UP, KeyEvent.KEYCODE_DPAD_DOWN,
                KeyEvent.KEYCODE_DPAD_LEFT, KeyEvent.KEYCODE_DPAD_RIGHT, KeyEvent.KEYCODE_DPAD_CENTER};
        int[] expected = {0x26, 0x28, 0x25, 0x27, 0x0d};
        for (int i = 0; i < buttons.length; i++) {
            assertTrue(game.handleKeyDown(event(KeyEvent.ACTION_DOWN, buttons[i], remote.getId(), InputDevice.SOURCE_DPAD)));
            assertTrue(game.handleKeyUp(event(KeyEvent.ACTION_UP, buttons[i], remote.getId(), InputDevice.SOURCE_DPAD)));
            assertEquals(expected[i] + ":" + KeyboardPacket.KEY_DOWN, connection.keys.get(i * 2));
            assertEquals(expected[i] + ":" + KeyboardPacket.KEY_UP, connection.keys.get(i * 2 + 1));
        }
        ReflectionHelpers.setField(game, "grabbedInput", false);
        game.handleKeyDown(event(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_CENTER, remote.getId(), InputDevice.SOURCE_DPAD));
        game.handleKeyUp(event(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_DPAD_CENTER, remote.getId(), InputDevice.SOURCE_DPAD));
        assertEquals(10, connection.keys.size());
    }

    @Test public void missingDeviceStillUsesRemoteSourceForLocalMenus() {
        RemoteGame game = game(new RecordingConnection());
        assertTrue(game.handleKeyDown(event(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_MENU, -123, InputDevice.SOURCE_DPAD)));
        assertTrue(game.handleKeyUp(event(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_MENU, -123, InputDevice.SOURCE_DPAD)));
        assertEquals(1, game.menus);
        assertFalse(ControllerHandler.isRemoteControlEvent(event(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_MENU,
                -123, InputDevice.SOURCE_GAMEPAD)));
    }

    @Test public void handheldBackIsOptInAndDoesNotStealSelectOrStart() {
        InputDevice pad = device(InputDevice.SOURCE_GAMEPAD | InputDevice.SOURCE_DPAD,
                InputDevice.KEYBOARD_TYPE_NON_ALPHABETIC);
        RecordingConnection connection = new RecordingConnection();
        RemoteGame game = game(connection);
        PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
        ReflectionHelpers.setField(game, "connected", true);
        CountingController controller = new CountingController(game, connection, prefs);
        ReflectionHelpers.setField(game, "controllerHandler", controller);
        try {
            assertTrue(game.handleKeyDown(event(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BACK, pad.getId(), InputDevice.SOURCE_GAMEPAD)));
            assertTrue(game.handleKeyUp(event(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BACK, pad.getId(), InputDevice.SOURCE_GAMEPAD)));
            assertEquals(2, controller.events);
            assertEquals(0, game.menus);
            prefs.backOpensMenu = true;
            assertTrue(game.handleKeyDown(event(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BACK, pad.getId(), InputDevice.SOURCE_GAMEPAD)));
            assertEquals(0, game.menus);
            assertTrue(game.handleKeyUp(event(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BACK, pad.getId(), InputDevice.SOURCE_GAMEPAD)));
            assertEquals(1, game.menus);
            assertEquals(2, controller.events);
            for (int key : new int[] {KeyEvent.KEYCODE_BUTTON_SELECT, KeyEvent.KEYCODE_BUTTON_START}) {
                assertTrue(game.handleKeyDown(event(KeyEvent.ACTION_DOWN, key, pad.getId(), InputDevice.SOURCE_GAMEPAD)));
                assertTrue(game.handleKeyUp(event(KeyEvent.ACTION_UP, key, pad.getId(), InputDevice.SOURCE_GAMEPAD)));
            }
            assertEquals(6, controller.events);
            assertEquals(1, game.menus);
        } finally {
            controller.destroy();
        }
    }

    @Test public void handheldBackOptionPreservesKeyboardWindowsKeyMode() {
        InputDevice keyboard = device(InputDevice.SOURCE_KEYBOARD, InputDevice.KEYBOARD_TYPE_ALPHABETIC);
        RecordingConnection connection = new RecordingConnection();
        RemoteGame game = game(connection);
        ReflectionHelpers.setField(game, "connected", true);
        PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
        prefs.backOpensMenu = prefs.backAsMeta = true;
        assertTrue(game.handleKeyDown(event(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BACK, keyboard.getId(), InputDevice.SOURCE_KEYBOARD)));
        assertTrue(game.handleKeyUp(event(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BACK, keyboard.getId(), InputDevice.SOURCE_KEYBOARD)));
        assertEquals(0, game.menus);
        assertEquals("91:" + KeyboardPacket.KEY_DOWN, connection.keys.get(0));
        assertEquals("91:" + KeyboardPacket.KEY_UP, connection.keys.get(1));
    }

    // #50: the Esc key of many keyboards made for Android sends Back. It goes to the PC as Esc by default
    // instead of opening the stream menu; with Send system keys off it is Android's Back again.
    // Only an external keyboard's Back becomes Esc: below Android 10 Nightjar cannot tell, and a keyboard
    // phone's own keyboard may lack the Ctrl for Ctrl+Alt+Shift+M, so Back stays Back.
    @Test public void keyboardBackSendsEscUnlessSystemKeysStayWithAndroid() {
        InputDevice keyboard = device(InputDevice.SOURCE_KEYBOARD, InputDevice.KEYBOARD_TYPE_ALPHABETIC);
        RecordingConnection connection = new RecordingConnection();
        RemoteGame game = game(connection);
        ReflectionHelpers.setField(game, "connected", true);
        PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
        assertTrue(prefs.captureSystemKeys);

        // A keyboard phone's built-in keyboard
        assertFalse(game.handleKeyDown(event(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BACK, keyboard.getId(), InputDevice.SOURCE_KEYBOARD)));
        assertTrue(connection.keys.isEmpty());

        ReflectionHelpers.setField(keyboard, "mIsExternal", true);
        if (android.os.Build.VERSION.SDK_INT < android.os.Build.VERSION_CODES.Q) {
            assertFalse(game.handleKeyDown(event(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BACK, keyboard.getId(), InputDevice.SOURCE_KEYBOARD)));
            assertTrue(connection.keys.isEmpty());
            return;
        }
        assertTrue(game.handleKeyDown(event(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BACK, keyboard.getId(), InputDevice.SOURCE_KEYBOARD)));
        assertTrue(game.handleKeyUp(event(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BACK, keyboard.getId(), InputDevice.SOURCE_KEYBOARD)));
        assertEquals(0, game.menus + game.backs);
        assertEquals("27:" + KeyboardPacket.KEY_DOWN, connection.keys.get(0));
        assertEquals("27:" + KeyboardPacket.KEY_UP, connection.keys.get(1));

        // Physical Back opens stream menu is an explicit choice and keeps keyboard Back.
        prefs.backOpensMenu = true;
        assertTrue(game.handleKeyDown(event(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BACK, keyboard.getId(), InputDevice.SOURCE_KEYBOARD)));
        assertTrue(game.handleKeyUp(event(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BACK, keyboard.getId(), InputDevice.SOURCE_KEYBOARD)));
        assertEquals(1, game.menus);
        prefs.backOpensMenu = false;

        prefs.captureSystemKeys = false;
        assertFalse(game.handleKeyDown(event(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BACK, keyboard.getId(), InputDevice.SOURCE_KEYBOARD)));
        assertEquals(2, connection.keys.size());
    }

    // #49: a handheld whose firmware injects Back for its system button sends Guide with Back button as
    // Guide on; Android's Back gesture still leaves the stream, and the option is off by default.
    @Test public void injectedHandheldBackSendsGuideOnlyWhenEnabled() {
        InputDevice injected = device(InputDevice.SOURCE_KEYBOARD, InputDevice.KEYBOARD_TYPE_ALPHABETIC);
        ReflectionHelpers.setField(injected, "mId", -1);
        RecordingConnection connection = new RecordingConnection();
        RemoteGame game = game(connection);
        PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
        ReflectionHelpers.setField(game, "connected", true);
        HandheldController controller = new HandheldController(game, connection, prefs);
        ReflectionHelpers.setField(ReflectionHelpers.getField(controller, "defaultContext"), "assignedControllerNumber", true);
        ReflectionHelpers.setField(game, "controllerHandler", controller);
        try {
            assertFalse(prefs.backAsGuide);
            assertFalse(game.handleKeyDown(event(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BACK, -1, InputDevice.SOURCE_KEYBOARD)));
            assertTrue(connection.buttons.isEmpty());

            prefs.backAsGuide = true;
            assertTrue(game.handleKeyDown(event(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BACK, -1, InputDevice.SOURCE_KEYBOARD)));
            assertEquals(com.limelight.nvstream.input.ControllerPacket.SPECIAL_BUTTON_FLAG,
                    (int) connection.buttons.get(connection.buttons.size() - 1));
            assertTrue(game.handleKeyUp(event(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BACK, -1, InputDevice.SOURCE_KEYBOARD)));
            assertEquals(0, (int) connection.buttons.get(connection.buttons.size() - 1));
            assertTrue(connection.keys.isEmpty());

            // Android's navigation bar and Back gesture are left to Android, which opens the stream menu.
            KeyEvent gesture = KeyEvent.changeFlags(event(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BACK, -1,
                    InputDevice.SOURCE_KEYBOARD), KeyEvent.FLAG_VIRTUAL_HARD_KEY);
            int sent = connection.buttons.size();
            assertFalse(game.handleKeyDown(gesture));
            assertEquals(sent, connection.buttons.size());
            assertEquals(0, game.menus + game.backs);
        } finally {
            controller.destroy();
        }
    }

    // #49: a handheld's system button on a built-in key device such as gpio-keys (internal, not a gamepad,
    // not a keyboard) sends Guide too; an external keyboard's Back does not.
    @Test public void builtInKeyDeviceBackSendsGuide() {
        InputDevice keys = device(InputDevice.SOURCE_KEYBOARD, InputDevice.KEYBOARD_TYPE_NON_ALPHABETIC);
        RecordingConnection connection = new RecordingConnection();
        RemoteGame game = game(connection);
        PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
        prefs.backAsGuide = true;
        ReflectionHelpers.setField(game, "connected", true);
        HandheldController controller = new HandheldController(game, connection, prefs);
        ReflectionHelpers.setField(ReflectionHelpers.getField(controller, "defaultContext"), "assignedControllerNumber", true);
        ReflectionHelpers.setField(game, "controllerHandler", controller);
        try {
            assertTrue(game.handleKeyDown(event(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BACK, keys.getId(), InputDevice.SOURCE_KEYBOARD)));
            assertEquals(com.limelight.nvstream.input.ControllerPacket.SPECIAL_BUTTON_FLAG,
                    (int) connection.buttons.get(connection.buttons.size() - 1));
            assertTrue(game.handleKeyUp(event(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BACK, keys.getId(), InputDevice.SOURCE_KEYBOARD)));
            assertEquals(0, (int) connection.buttons.get(connection.buttons.size() - 1));

            InputDevice keyboard = device(InputDevice.SOURCE_KEYBOARD, InputDevice.KEYBOARD_TYPE_ALPHABETIC);
            int sent = connection.buttons.size();
            assertFalse(controller.handleHandheldBackAsGuide(event(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BACK,
                    keyboard.getId(), InputDevice.SOURCE_KEYBOARD)));
            assertEquals(sent, connection.buttons.size());
        } finally {
            controller.destroy();
        }
    }

    private static class HandheldController extends ControllerHandler {
        private final boolean internalSelect;
        HandheldController(Game activity, NvConnection connection, PreferenceConfiguration prefs) {
            this(activity, connection, prefs, true);
        }
        HandheldController(Game activity, NvConnection connection, PreferenceConfiguration prefs, boolean internalSelect) {
            super(activity, connection, null, prefs);
            this.internalSelect = internalSelect;
        }
        @Override protected boolean[] findInternalGamepadAndSelect() { return new boolean[] {true, internalSelect}; }
    }

    // #49: on a handheld whose built-in pad has no Select (Shield Portable), the system Back is that pad's
    // Select (shouldIgnoreBack), so Back as Guide leaves it alone and Select and the menu chord keep working.
    @Test public void handheldWithoutItsOwnSelectKeepsBackAsSelect() {
        InputDevice keys = device(InputDevice.SOURCE_KEYBOARD, InputDevice.KEYBOARD_TYPE_NON_ALPHABETIC);
        RecordingConnection connection = new RecordingConnection();
        RemoteGame game = game(connection);
        PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
        prefs.backAsGuide = true;
        ReflectionHelpers.setField(game, "connected", true);
        HandheldController controller = new HandheldController(game, connection, prefs, false);
        ReflectionHelpers.setField(ReflectionHelpers.getField(controller, "defaultContext"), "assignedControllerNumber", true);
        try {
            assertFalse(controller.handleHandheldBackAsGuide(event(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BACK,
                    keys.getId(), InputDevice.SOURCE_KEYBOARD)));
            assertFalse(controller.handleHandheldBackAsGuide(event(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BACK,
                    -1, InputDevice.SOURCE_KEYBOARD)));
            assertTrue(connection.buttons.isEmpty());
        } finally {
            controller.destroy();
        }
    }

    private static class CountingController extends ControllerHandler {
        int events;
        CountingController(Game activity, NvConnection connection, PreferenceConfiguration prefs) {
            super(activity, connection, null, prefs);
        }
        @Override public boolean handleButtonDown(KeyEvent event) { events++; return true; }
        @Override public boolean handleButtonUp(KeyEvent event) { events++; return true; }
    }

    private static class RecordingConnection extends NvConnection {
        final List<String> keys = new ArrayList<>();
        final List<Integer> buttons = new ArrayList<>();
        RecordingConnection() {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }
        @Override public void sendKeyboardInput(short key, byte direction, byte modifiers, byte flags) {
            keys.add((key & 0xff) + ":" + direction);
        }
        @Override public void sendControllerInput(short number, short mask, int buttons, byte leftTrigger,
                                                   byte rightTrigger, short leftX, short leftY, short rightX, short rightY) {
            this.buttons.add(buttons);
        }
    }
}

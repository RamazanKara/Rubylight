package com.limelight;

import android.app.Application;
import android.hardware.input.InputManager;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.MotionEvent;

import com.limelight.GameKeyboardOverlayTest.RecordingConnection;
import com.limelight.binding.input.keyboard.KeyboardFocus;
import com.limelight.binding.input.keyboard.PcKey;
import com.limelight.binding.input.keyboard.PcKeyboardController;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import static com.limelight.GameKeyboardOverlayTest.*;
import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

/**
 * A USB or Bluetooth pad steering the on-screen keyboard, with synthesized key and motion events carrying
 * gamepad sources. The pads' behaviour is from Android's documented input model; none of it has run on a
 * real USB pad (see docs/gamepad-navigation.md).
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GamePadKeyboardNavigationTest {
    private static final int OTHER_PAD_ID = 31;
    private static long clock = 10_000;

    /** A joystick motion event as a pad delivers it: hat and stick axes on a SOURCE_JOYSTICK event. */
    private static MotionEvent joystick(int deviceId, int action, float hatX, float hatY, float stickX, float stickY) {
        MotionEvent.PointerProperties properties = new MotionEvent.PointerProperties();
        properties.id = 0;
        properties.toolType = MotionEvent.TOOL_TYPE_UNKNOWN;
        MotionEvent.PointerCoords coords = new MotionEvent.PointerCoords();
        coords.setAxisValue(MotionEvent.AXIS_HAT_X, hatX);
        coords.setAxisValue(MotionEvent.AXIS_HAT_Y, hatY);
        coords.setAxisValue(MotionEvent.AXIS_X, stickX);
        coords.setAxisValue(MotionEvent.AXIS_Y, stickY);
        clock += 5;
        return MotionEvent.obtain(clock, clock, action, 1, new MotionEvent.PointerProperties[] {properties},
                new MotionEvent.PointerCoords[] {coords}, 0, 0, 1, 1, deviceId, 0,
                InputDevice.SOURCE_JOYSTICK, 0);
    }

    private static boolean hat(Game game, int deviceId, float x, float y) {
        return game.onGenericMotionEvent(joystick(deviceId, MotionEvent.ACTION_MOVE, x, y, 0, 0));
    }

    private static Game openKeyboard(RecordingConnection connection) {
        Game game = pcKeyboardGame(connection);
        ReflectionHelpers.callInstanceMethod(game, "togglePcKeyboard");
        // Touch hides the selection until a pad shows it; start from a pad that already did.
        PcKeyboardController keyboard = pcKeyboardController(game);
        assertTrue(keyboard.isFocusVisible());
        return game;
    }

    private static int column(Game game) {
        return pcKeyboardController(game).focus().column();
    }

    private static KeyEvent key(int action, int keyCode, int repeat, int flags, InputDevice device, int source) {
        KeyEvent event = new KeyEvent(0, 0, action, keyCode, repeat, 0, device.getId(), 0, flags, source);
        shadowOf(event).setDevice(device);
        return event;
    }

    private static InputDevice device(int id, int sources) {
        InputDevice device = gamepad();
        ReflectionHelpers.setField(device, "mId", id);
        ReflectionHelpers.setField(device, "mSources", sources);
        return device;
    }

    @Test
    public void aHatPushMovesTheSelectionOnceAndTheMotionIsConsumed() {
        Game game = openKeyboard(new RecordingConnection());
        int start = column(game);
        assertTrue("The hat is the keyboard's, not the game's", hat(game, PAD_ID, 1, 0));
        assertEquals(start + 1, column(game));
        // More samples of the same push do not move again; letting go does not either.
        assertTrue(hat(game, PAD_ID, 1, 0));
        assertEquals(start + 1, column(game));
        assertTrue(hat(game, PAD_ID, 0, 0));
        assertEquals(start + 1, column(game));
        assertFalse(pcKeyboardController(game).needsTick());
    }

    @Test
    public void aHeldHatRepeatsAtTheKeyboardsOwnPace() {
        Game game = openKeyboard(new RecordingConnection());
        PcKeyboardController keyboard = pcKeyboardController(game);
        int start = column(game);
        long t0 = clock + 5;
        hat(game, PAD_ID, 1, 0);
        assertEquals(start + 1, column(game));
        assertTrue(keyboard.needsTick());
        keyboard.tick(t0 + PcKeyboardController.NAVIGATE_DELAY_MS - 1);
        assertEquals("Nothing before the delay", start + 1, column(game));
        keyboard.tick(t0 + PcKeyboardController.NAVIGATE_DELAY_MS + 5);
        assertEquals(start + 2, column(game));
        keyboard.tick(t0 + PcKeyboardController.NAVIGATE_DELAY_MS + 5 + PcKeyboardController.NAVIGATE_REPEAT_MS);
        assertEquals(start + 3, column(game));
        hat(game, PAD_ID, 0, 0);
        assertFalse(keyboard.needsTick());
    }

    @Test
    public void aPadThatReportsTheDpadAsKeysAndAsAHatMovesOnce() {
        Game game = openKeyboard(new RecordingConnection());
        int start = column(game);
        // Hat first, then the key for the same press.
        hat(game, PAD_ID, 1, 0);
        assertTrue(padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_RIGHT));
        assertEquals("One press, one move", start + 1, column(game));
        // The key comes up first while the hat is still down: still no extra move, still held.
        assertTrue(padKey(game, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_DPAD_RIGHT));
        assertEquals(start + 1, column(game));
        assertTrue(pcKeyboardController(game).needsTick());
        hat(game, PAD_ID, 0, 0);
        assertFalse(pcKeyboardController(game).needsTick());

        // And the other order: key first, then the hat.
        assertTrue(padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_RIGHT));
        hat(game, PAD_ID, 1, 0);
        assertEquals(start + 2, column(game));
        hat(game, PAD_ID, 0, 0);
        assertTrue(pcKeyboardController(game).needsTick());
        assertTrue(padKey(game, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_DPAD_RIGHT));
        assertFalse(pcKeyboardController(game).needsTick());
        assertEquals(start + 2, column(game));
    }

    @Test
    public void keyRepeatsFromTheSystemDoNotMoveTheSelectionAgain() {
        Game game = openKeyboard(new RecordingConnection());
        int start = column(game);
        InputDevice pad = gamepad();
        assertTrue(game.handleKeyDown(key(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_RIGHT, 0, 0, pad, InputDevice.SOURCE_GAMEPAD)));
        for (int repeat = 1; repeat <= 5; repeat++) {
            assertTrue(game.handleKeyDown(key(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_RIGHT, repeat, 0, pad, InputDevice.SOURCE_GAMEPAD)));
        }
        assertEquals("The keyboard repeats by itself", start + 1, column(game));
        assertTrue(game.handleKeyUp(key(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_DPAD_RIGHT, 0, 0, pad, InputDevice.SOURCE_GAMEPAD)));
        assertFalse(pcKeyboardController(game).needsTick());
    }

    @Test
    public void unpluggingAPadHoldingTheHatStopsTheSelectionWalking() {
        Game game = openKeyboard(new RecordingConnection());
        PcKeyboardController keyboard = pcKeyboardController(game);
        hat(game, PAD_ID, 1, 0);
        assertTrue(keyboard.needsTick());
        InputManager.InputDeviceListener listener = ReflectionHelpers.getField(game, "pcKeyboardDeviceListener");
        listener.onInputDeviceRemoved(PAD_ID);
        assertFalse("The release will never come", keyboard.needsTick());
        assertFalse(keyboard.stickActive());
        // A new pad pushing the same way starts a fresh move.
        int before = column(game);
        hat(game, OTHER_PAD_ID, 1, 0);
        assertEquals(before + 1, column(game));
    }

    @Test
    public void aCancelledMotionStreamReleasesTheHat() {
        Game game = openKeyboard(new RecordingConnection());
        PcKeyboardController keyboard = pcKeyboardController(game);
        hat(game, PAD_ID, 0, 1);
        assertTrue(keyboard.needsTick());
        game.onGenericMotionEvent(joystick(PAD_ID, MotionEvent.ACTION_CANCEL, 0, 0, 0, 0));
        assertFalse(keyboard.needsTick());
    }

    @Test
    public void aSecondPadAtRestDoesNotCancelTheFirstPadsHeldHat() {
        Game game = openKeyboard(new RecordingConnection());
        PcKeyboardController keyboard = pcKeyboardController(game);
        hat(game, PAD_ID, 1, 0);
        // Stick noise from another pad, well inside the dead zone.
        game.onGenericMotionEvent(joystick(OTHER_PAD_ID, MotionEvent.ACTION_MOVE, 0, 0, 0.05f, -0.02f));
        assertTrue("Still held", keyboard.stickActive());
        // The second pad pushing takes over; the first releasing afterwards must not cancel it.
        int before = column(game);
        game.onGenericMotionEvent(joystick(OTHER_PAD_ID, MotionEvent.ACTION_MOVE, 0, 0, -0.9f, 0));
        assertEquals(before - 1, column(game));
        hat(game, PAD_ID, 0, 0);
        assertTrue(keyboard.stickActive());
        game.onGenericMotionEvent(joystick(OTHER_PAD_ID, MotionEvent.ACTION_MOVE, 0, 0, 0, 0));
        assertFalse(keyboard.needsTick());
    }

    @Test
    public void theLeftStickSteersAndAHatTakesOverAxisByAxis() {
        Game game = openKeyboard(new RecordingConnection());
        int start = column(game);
        game.onGenericMotionEvent(joystick(PAD_ID, MotionEvent.ACTION_MOVE, 0, 0, 0.9f, 0.1f));
        assertEquals(start + 1, column(game));
        game.onGenericMotionEvent(joystick(PAD_ID, MotionEvent.ACTION_MOVE, 0, 0, 0, 0));
        // Pushing past 0.6 engages; a stick that merely drifts does not.
        game.onGenericMotionEvent(joystick(PAD_ID, MotionEvent.ACTION_MOVE, 0, 0, 0.5f, 0.2f));
        assertEquals(start + 1, column(game));
    }

    @Test
    public void closingTheKeyboardWithAHatHeldLeavesNothingWalking() {
        Game game = openKeyboard(new RecordingConnection());
        PcKeyboardController keyboard = pcKeyboardController(game);
        hat(game, PAD_ID, -1, 0);
        ReflectionHelpers.callInstanceMethod(game, "togglePcKeyboard");
        assertFalse(pcKeyboardVisible(game));
        assertFalse(keyboard.needsTick());
        assertEquals("No axis owner is kept", -1, (int) ReflectionHelpers.getField(game, "pcKeyboardAxisDevice"));
    }

    @Test
    public void flippedFaceButtonsForANintendoLayoutSwapConfirmAndBack() {
        RecordingConnection connection = new RecordingConnection();
        Game game = openKeyboard(connection);
        com.limelight.preferences.PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
        prefs.flipFaceButtons = true;
        PcKey focused = pcKeyboardController(game).focus().focused();
        connection.keys.clear();
        // KEYCODE_BUTTON_A is the south button on every pad, so with the flip it is "back" and the east
        // button, KEYCODE_BUTTON_B, types: the Nintendo order, where A is on the right.
        assertTrue(padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_B));
        assertTrue(padKey(game, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_B));
        assertEquals(2, connection.keys.size());
        assertEquals(0x8000 | focused.vk, connection.keys.get(0)[0]);
        assertTrue(pcKeyboardVisible(game));
        assertTrue(padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_A));
        assertFalse(pcKeyboardVisible(game));
        assertTrue(padKey(game, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_A));
    }

    @Test
    public void aPadsOwnBackKeyClosesTheKeyboard() {
        RecordingConnection connection = new RecordingConnection();
        Game game = openKeyboard(connection);
        connection.keys.clear();
        assertTrue(padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BACK));
        assertFalse(pcKeyboardVisible(game));
        assertTrue("The release stays with the keyboard", padKey(game, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BACK));
        assertTrue("Back never types", connection.keys.isEmpty());
    }

    @Test
    public void systemButtonsOnCommonPadsAreSwallowedAndSendNothing() {
        RecordingConnection connection = new RecordingConnection();
        Game game = openKeyboard(connection);
        connection.keys.clear();
        // Select (Xbox View, DualShock and DualSense Share), Mode (Guide, PS), Button 1 (the DualShock 4
        // touchpad click as Android remaps it), both sticks, the triggers' digital keys and the rare C and Z.
        int[] keys = {KeyEvent.KEYCODE_BUTTON_SELECT, KeyEvent.KEYCODE_BUTTON_MODE, KeyEvent.KEYCODE_BUTTON_1,
                KeyEvent.KEYCODE_BUTTON_THUMBL, KeyEvent.KEYCODE_BUTTON_THUMBR,
                KeyEvent.KEYCODE_BUTTON_L2, KeyEvent.KEYCODE_BUTTON_R2, KeyEvent.KEYCODE_BUTTON_C,
                KeyEvent.KEYCODE_BUTTON_Z};
        for (int keyCode : keys) {
            assertTrue("down " + keyCode, padKey(game, KeyEvent.ACTION_DOWN, keyCode));
            assertTrue("up " + keyCode, padKey(game, KeyEvent.ACTION_UP, keyCode));
        }
        assertTrue(connection.keys.isEmpty());
        assertTrue(pcKeyboardVisible(game));
        assertEquals(0, heldPadKeys(game).size());
    }

    @Test
    public void aJoystickOnlyDeviceCountsAsAPadButAKeyboardWithArrowsDoesNot() {
        RecordingConnection connection = new RecordingConnection();
        Game game = openKeyboard(connection);
        int start = column(game);
        // Some generic HID pads advertise SOURCE_JOYSTICK without SOURCE_GAMEPAD.
        InputDevice joystickOnly = device(40, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_DPAD);
        assertTrue(takenByPcKeyboard(game, key(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_RIGHT, 0, 0, joystickOnly,
                InputDevice.SOURCE_DPAD)));
        assertEquals(start + 1, column(game));
        assertTrue(takenByPcKeyboard(game, key(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_DPAD_RIGHT, 0, 0, joystickOnly,
                InputDevice.SOURCE_DPAD)));
        // A physical keyboard keeps typing to the PC and does not steer the on-screen one.
        InputDevice keyboard = device(41, InputDevice.SOURCE_KEYBOARD | InputDevice.SOURCE_DPAD);
        ReflectionHelpers.setField(keyboard, "mKeyboardType", InputDevice.KEYBOARD_TYPE_ALPHABETIC);
        assertFalse(takenByPcKeyboard(game, key(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_RIGHT, 0, 0, keyboard,
                InputDevice.SOURCE_KEYBOARD)));
        assertEquals(start + 1, column(game));
    }

    @Test
    public void directionNamesComeFromTheHatsSignsAsAndroidReportsThem() {
        // AXIS_HAT_Y is -1 for up and +1 for down; AXIS_HAT_X is -1 for left and +1 for right.
        assertEquals(KeyboardFocus.Direction.UP, PcKeyboardController.stickDirection(0, -1, null));
        assertEquals(KeyboardFocus.Direction.DOWN, PcKeyboardController.stickDirection(0, 1, null));
        assertEquals(KeyboardFocus.Direction.LEFT, PcKeyboardController.stickDirection(-1, 0, null));
        assertEquals(KeyboardFocus.Direction.RIGHT, PcKeyboardController.stickDirection(1, 0, null));
        // A diagonal hat (both axes at 1) picks the vertical one deterministically instead of both.
        assertEquals(KeyboardFocus.Direction.DOWN, PcKeyboardController.stickDirection(1, 1, null));
    }
}

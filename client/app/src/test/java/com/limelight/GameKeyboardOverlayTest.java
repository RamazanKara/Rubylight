package com.limelight;

import android.app.Application;
import android.content.ComponentName;
import android.content.Context;
import android.graphics.Insets;
import android.hardware.input.InputManager;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.View;
import android.view.WindowInsets;
import android.view.WindowManager;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputMethodManager;
import android.widget.FrameLayout;

import com.limelight.binding.input.ControllerButtonMap;
import com.limelight.binding.input.ControllerHandler;
import com.limelight.binding.input.KeyboardTranslator;
import com.limelight.binding.input.keyboard.PcKey;
import com.limelight.binding.input.keyboard.PcKeyboardController;
import com.limelight.binding.input.keyboard.PcKeyboardLayout;
import com.limelight.binding.input.keyboard.PcModifier;
import com.limelight.binding.input.capture.InputCaptureProvider;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.input.KeyboardPacket;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.PcKeyboardOverlay;
import com.limelight.ui.StreamView;
import com.limelight.ui.StreamMenu;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowInputDevice;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameKeyboardOverlayTest {
    @Test
    @Config(sdk = {29, 36}, qualifiers = "sw800dp-land")
    public void tabletStreamDoesNotAutomaticallyOpenSoftwareKeyboard() throws Exception {
        Game game = game(new RecordingConnection());
        int softInputMode = game.getPackageManager().getActivityInfo(
                new ComponentName(game, Game.class), 0).softInputMode;
        // StreamView must retain text-editor support for the explicit keyboard action, but
        // its initial focus must not trigger Android's large-screen automatic IME policy.
        assertEquals(WindowManager.LayoutParams.SOFT_INPUT_STATE_HIDDEN,
                softInputMode & WindowManager.LayoutParams.SOFT_INPUT_MASK_STATE);
        assertTrue(new StreamView(game).onCheckIsTextEditor());
    }

    @Test
    public void phoneKeyboardMenuActionHidesDesktopPanelAndStillSendsText() {
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        game.setTheme(R.style.AppTheme);
        StreamView stream = new StreamView(game);
        stream.setFocusableInTouchMode(true);
        stream.setInputCallbacks(game);
        FrameLayout parent = new FrameLayout(game);
        parent.addView(stream);
        game.setContentView(parent);
        ReflectionHelpers.setField(game, "streamView", stream);
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "toggleKeyboardOverlay");
        View panel = ReflectionHelpers.getField(game, "keyboardOverlay");
        assertEquals(View.VISIBLE, panel.getVisibility());
        InputMethodManager input = (InputMethodManager) game.getSystemService(Context.INPUT_METHOD_SERVICE);
        assertFalse(shadowOf(input).isSoftInputVisible());

        ReflectionHelpers.callInstanceMethod(game.menuActions, "onStreamMenuItem",
                ReflectionHelpers.ClassParameter.from(StreamMenu.Item.class, StreamMenu.Item.KEYBOARD));
        Runnable action = ReflectionHelpers.getField(game, "streamMenuAction");
        assertNotNull(action);
        // The menu hands the selected action back after its window closes.
        action.run();

        assertEquals(View.GONE, panel.getVisibility());
        assertTrue(shadowOf(input).isSoftInputVisible());
        assertTrue(stream.hasFocus());
        InputConnection editor = stream.onCreateInputConnection(new EditorInfo());
        assertNotNull(editor);
        assertTrue(editor.commitText("Hello, world!", 1));
        assertEquals(java.util.Collections.singletonList("Hello, world!"), connection.text);
    }

    @Test
    public void closingPhoneKeyboardDoesNotHideDesktopPanel() {
        Game game = game(new RecordingConnection());
        game.setTheme(R.style.AppTheme);
        StreamView stream = new StreamView(game) {
            @Override public WindowInsets getRootWindowInsets() {
                // API 29 derives IME visibility from the bottom inset beyond stable bars.
                return new WindowInsets.Builder().setSystemWindowInsets(Insets.of(0, 0, 0, 300)).build();
            }
        };
        stream.setFocusableInTouchMode(true);
        FrameLayout parent = new FrameLayout(game);
        parent.addView(stream);
        game.setContentView(parent);
        ReflectionHelpers.setField(game, "streamView", stream);
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "toggleKeyboardOverlay");
        View panel = ReflectionHelpers.getField(game, "keyboardOverlay");
        InputMethodManager input = (InputMethodManager) game.getSystemService(Context.INPUT_METHOD_SERVICE);
        input.showSoftInput(stream, 0);
        assertTrue(shadowOf(input).isSoftInputVisible());

        game.toggleKeyboard();

        assertFalse(shadowOf(input).isSoftInputVisible());
        assertEquals(View.VISIBLE, panel.getVisibility());
    }

    @Test
    public void taskManagerShortcutReleasesKeysInReverseOrder() {
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        sendShortcut(game, KeyEvent.KEYCODE_CTRL_LEFT, KeyEvent.KEYCODE_SHIFT_LEFT, KeyEvent.KEYCODE_ESCAPE);
        KeyboardTranslator translator = new KeyboardTranslator();
        int ctrl = translator.translate(KeyEvent.KEYCODE_CTRL_LEFT, -1) & 0xffff;
        int shift = translator.translate(KeyEvent.KEYCODE_SHIFT_LEFT, -1) & 0xffff;
        int escape = translator.translate(KeyEvent.KEYCODE_ESCAPE, -1) & 0xffff;
        int modifiers = KeyboardPacket.MODIFIER_CTRL | KeyboardPacket.MODIFIER_SHIFT;
        assertEquals(6, connection.keys.size());
        assertArrayEquals(new int[] {ctrl, KeyboardPacket.KEY_DOWN, KeyboardPacket.MODIFIER_CTRL}, connection.keys.get(0));
        assertArrayEquals(new int[] {shift, KeyboardPacket.KEY_DOWN, modifiers}, connection.keys.get(1));
        assertArrayEquals(new int[] {escape, KeyboardPacket.KEY_DOWN, modifiers}, connection.keys.get(2));
        assertArrayEquals(new int[] {escape, KeyboardPacket.KEY_UP, modifiers}, connection.keys.get(3));
        assertArrayEquals(new int[] {shift, KeyboardPacket.KEY_UP, KeyboardPacket.MODIFIER_CTRL}, connection.keys.get(4));
        assertArrayEquals(new int[] {ctrl, KeyboardPacket.KEY_UP, 0}, connection.keys.get(5));
        Map<Short, Byte> pressed = ReflectionHelpers.getField(game.keyboardInput, "pressedKeys");
        assertTrue(pressed.isEmpty());
    }

    @Test
    public void aLocalChordKeepsItsModifierTapsFromBeingLoneTapsOnTheHost() {
        RecordingConnection connection = new RecordingConnection();
        Game game = pcKeyboardGame(connection);
        int neutral = 0x8000 | com.limelight.binding.input.keyboard.PcKeyboardEngine.NEUTRAL_VK;
        int[] chord = {KeyEvent.KEYCODE_CTRL_LEFT, KeyEvent.KEYCODE_ALT_LEFT, KeyEvent.KEYCODE_SHIFT_LEFT};
        for (int key : chord) game.handleKeyDown(keyboardKey(KeyEvent.ACTION_DOWN, key));
        assertEquals("the modifiers are sent as they go down", 3, connection.keys.size());
        assertTrue(game.handleKeyDown(keyboardKey(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_K)));
        assertEquals("the shortcut key itself never reaches the host", 5, connection.keys.size());
        assertEquals(neutral, connection.keys.get(3)[0]);
        assertEquals(KeyboardPacket.KEY_DOWN, connection.keys.get(3)[1]);
        assertEquals(neutral, connection.keys.get(4)[0]);
        assertEquals(KeyboardPacket.KEY_UP, connection.keys.get(4)[1]);
        assertTrue(game.handleKeyUp(keyboardKey(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_K)));
        for (int key : chord) game.handleKeyUp(keyboardKey(KeyEvent.ACTION_UP, key));
        assertTrue("the PC keyboard opened", pcKeyboardVisible(game));
        assertEquals("the modifier releases follow the neutral key", 8, connection.keys.size());
    }

    @Test
    public void aLocalChordSendsNothingExtraWhileInputIsNotGrabbed() {
        RecordingConnection connection = new RecordingConnection();
        Game game = pcKeyboardGame(connection);
        ReflectionHelpers.setField(game, "grabbedInput", false);
        for (int key : new int[] {KeyEvent.KEYCODE_CTRL_LEFT, KeyEvent.KEYCODE_ALT_LEFT, KeyEvent.KEYCODE_SHIFT_LEFT, KeyEvent.KEYCODE_K}) {
            game.handleKeyDown(keyboardKey(KeyEvent.ACTION_DOWN, key));
        }
        assertEquals(0, connection.keys.size());
    }

    @Test
    public void shortcutsPreservePhysicalModifiersAndRespectInputGrab() {
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        short ctrl = new KeyboardTranslator().translate(KeyEvent.KEYCODE_CTRL_LEFT, -1);
        Map<Short, Byte> pressed = ReflectionHelpers.getField(game.keyboardInput, "pressedKeys");
        pressed.put(ctrl, (byte) 0);
        ReflectionHelpers.setField(game, "modifierFlags", (int) KeyboardPacket.MODIFIER_CTRL);
        sendShortcut(game, KeyEvent.KEYCODE_CTRL_LEFT, KeyEvent.KEYCODE_SHIFT_LEFT, KeyEvent.KEYCODE_ESCAPE);
        assertEquals(4, connection.keys.size());
        assertEquals(1, pressed.size());
        assertTrue(pressed.containsKey(ctrl));
        assertEquals(KeyboardPacket.MODIFIER_CTRL, connection.keys.get(3)[2]);
        ReflectionHelpers.setField(game, "grabbedInput", false);
        sendShortcut(game, KeyEvent.KEYCODE_ALT_LEFT, KeyEvent.KEYCODE_F4);
        assertEquals(4, connection.keys.size());
        ReflectionHelpers.setField(game, "grabbedInput", true);
        ReflectionHelpers.setField(game, "foreground", false);
        sendShortcut(game, KeyEvent.KEYCODE_ESCAPE);
        assertEquals(4, connection.keys.size());
        ReflectionHelpers.setField(game, "foreground", true);
        connection.inputAllowed = false;
        sendShortcut(game, KeyEvent.KEYCODE_ESCAPE);
        assertEquals(4, connection.keys.size());
    }

    @Test
    public void toolbarReleasesPointerCaptureAndRestoresCursorPreference() {
        for (boolean visible : new boolean[] {false, true}) {
            Game game = game(new RecordingConnection());
            game.setTheme(R.style.AppTheme);
            StreamView stream = new StreamView(game);
            FrameLayout parent = new FrameLayout(game);
            parent.addView(stream);
            ReflectionHelpers.setField(game, "streamView", stream);
            Capture capture = new Capture();
            ReflectionHelpers.setField(game, "inputCaptureProvider", capture);
            ReflectionHelpers.setField(game, "cursorVisible", visible);
            ReflectionHelpers.callInstanceMethod(game.keyboardInput, "toggleKeyboardOverlay");
            assertTrue(capture.visible());
            assertTrue((boolean) ReflectionHelpers.getField(game, "grabbedInput"));
            ReflectionHelpers.callInstanceMethod(game.touchInput, "setInputGrabState",
                    ReflectionHelpers.ClassParameter.from(boolean.class, true));
            assertTrue(capture.visible());
            ReflectionHelpers.callInstanceMethod(game.keyboardInput, "hideKeyboardOverlay");
            assertEquals(visible, capture.visible());
            assertTrue((boolean) ReflectionHelpers.getField(game, "grabbedInput"));
        }
    }

    static Game pcKeyboardGame(RecordingConnection connection) {
        Game game = game(connection);
        game.setTheme(R.style.AppTheme);
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.pcKeyboardLayout = "us";
        prefs.pcKeyboardSize = 50;
        prefs.pcKeyboardOpacity = 90;
        ReflectionHelpers.setField(game, "prefConfig", prefs);
        ReflectionHelpers.setField(game, "controllerHandler", new ControllerHandler(game, connection, game, prefs));
        StreamView stream = new StreamView(game);
        stream.setFocusableInTouchMode(true);
        FrameLayout parent = new FrameLayout(game);
        parent.addView(stream);
        game.setContentView(parent);
        ReflectionHelpers.setField(game, "streamView", stream);
        return game;
    }

    static boolean pcKeyboardVisible(Game game) {
        return (boolean) ReflectionHelpers.callInstanceMethod(game.keyboardInput, "isPcKeyboardVisible");
    }

    static PcKeyboardController pcKeyboardController(Game game) {
        PcKeyboardOverlay overlay = ReflectionHelpers.getField(game, "pcKeyboard");
        return overlay.controller();
    }

    static final int PAD_ID = 12;

    static InputDevice gamepad() {
        InputDevice device = ShadowInputDevice.makeInputDeviceNamed("Pad");
        ReflectionHelpers.setField(device, "mId", PAD_ID);
        ReflectionHelpers.setField(device, "mSources", InputDevice.SOURCE_GAMEPAD | InputDevice.SOURCE_DPAD);
        ReflectionHelpers.setField(device, "mKeyboardType", InputDevice.KEYBOARD_TYPE_NON_ALPHABETIC);
        ReflectionHelpers.setField(device, "mMotionRanges", new java.util.ArrayList<>());
        return device;
    }

    static KeyEvent padEvent(int action, int keyCode, InputDevice device, int deviceId) {
        KeyEvent event = new KeyEvent(0, 0, action, keyCode, 0, 0, deviceId, 0, 0, InputDevice.SOURCE_GAMEPAD);
        shadowOf(event).setDevice(device);
        return event;
    }

    static boolean padKey(Game game, int action, int keyCode) {
        KeyEvent event = padEvent(action, keyCode, gamepad(), PAD_ID);
        return action == KeyEvent.ACTION_DOWN ? game.handleKeyDown(event) : game.handleKeyUp(event);
    }

    static boolean takenByPcKeyboard(Game game, KeyEvent event) {
        return (boolean) ReflectionHelpers.callInstanceMethod(game.keyboardInput, "handlePcKeyboardKey",
                ReflectionHelpers.ClassParameter.from(KeyEvent.class, event));
    }

    static android.util.SparseBooleanArray heldPadKeys(Game game) {
        return ReflectionHelpers.getField(game.keyboardInput, "pcKeyboardKeys");
    }

    static int[] lastKey(RecordingConnection connection) {
        return connection.keys.get(connection.keys.size() - 1);
    }

    @Test
    public void pcKeyboardOpensAndClosesAndExcludesTheOtherKeyboards() {
        Game game = pcKeyboardGame(new RecordingConnection());
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        assertTrue(pcKeyboardVisible(game));
        // The extra-keys panel takes over: only one keyboard at a time.
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "toggleKeyboardOverlay");
        assertFalse(pcKeyboardVisible(game));
        View panel = ReflectionHelpers.getField(game, "keyboardOverlay");
        assertEquals(View.VISIBLE, panel.getVisibility());
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        assertTrue(pcKeyboardVisible(game));
        assertEquals(View.GONE, panel.getVisibility());
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        assertFalse(pcKeyboardVisible(game));
    }

    @Test
    public void closingThePcKeyboardLetsGoOfEveryHeldAndStickyKey() {
        RecordingConnection connection = new RecordingConnection();
        Game game = pcKeyboardGame(connection);
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        PcKeyboardController keyboard = pcKeyboardController(game);
        PcKeyboardLayout layout = keyboard.layout();
        long now = 1000;
        // Ctrl locked, Shift armed, and A held down.
        keyboard.engine().tapModifier(PcModifier.CTRL);
        keyboard.engine().tapModifier(PcModifier.CTRL);
        keyboard.engine().tapModifier(PcModifier.SHIFT);
        keyboard.touchDown(layout.key("A"), 1, now);
        Map<Short, Byte> pressed = ReflectionHelpers.getField(game.keyboardInput, "pressedKeys");
        assertFalse(pressed.isEmpty());

        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");

        assertFalse(pcKeyboardVisible(game));
        assertTrue("Nothing stays pressed on the host", pressed.isEmpty());
        int ups = 0, downs = 0;
        for (int[] key : connection.keys) {
            if (key[1] == KeyboardPacket.KEY_DOWN) downs++; else ups++;
        }
        assertEquals("Every key sent down is sent up", downs, ups);
        assertTrue(downs >= 3);
    }

    @Test
    public void controllerKeysSteerTheKeyboardWhileOpenAndReachTheGameAfterwards() {
        RecordingConnection connection = new RecordingConnection();
        Game game = pcKeyboardGame(connection);
        padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_X);
        padKey(game, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_X);
        assertTrue("A closed keyboard sends no keys", connection.keys.isEmpty());

        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        connection.keys.clear();
        // X is Backspace, Y is Space: pressed and released on the host, and consumed.
        assertTrue(padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_X));
        assertTrue(padKey(game, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_X));
        assertTrue(padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_Y));
        assertTrue(padKey(game, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_Y));
        assertEquals(4, connection.keys.size());
        assertEquals(KeyboardPacket.KEY_DOWN, connection.keys.get(0)[1]);
        assertEquals(KeyboardPacket.KEY_UP, connection.keys.get(1)[1]);
        // Buttons without a job are swallowed too, so the game never sees them.
        assertTrue(padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_THUMBL));
        assertTrue(padKey(game, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_THUMBL));
        assertEquals(4, connection.keys.size());

        // B closes it. The release that follows still belongs to the keyboard.
        assertTrue(padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_B));
        assertTrue(padKey(game, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_B));
        assertFalse(pcKeyboardVisible(game));
        padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_X);
        padKey(game, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_X);
        assertEquals("Closed again: the pad no longer types", 4, connection.keys.size());
    }

    @Test
    public void closeWithAButtonHeldStillSwallowsItsRelease() {
        Game game = pcKeyboardGame(new RecordingConnection());
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        assertTrue(padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_Y));
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        assertFalse(pcKeyboardVisible(game));
        assertTrue("The game never saw the press, so it must not see the release",
                padKey(game, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_Y));
        assertEquals(0, heldPadKeys(game).size());
    }

    @Test
    public void noDeviceAndTheSyntheticDeviceAreNotControllers() {
        RecordingConnection connection = new RecordingConnection();
        Game game = pcKeyboardGame(connection);
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        connection.keys.clear();
        PcKey focused = pcKeyboardController(game).focus().focused();
        assertFalse(takenByPcKeyboard(game, padEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_Y, null, PAD_ID)));
        InputDevice synthetic = gamepad();
        ReflectionHelpers.setField(synthetic, "mId", -1);
        assertFalse(takenByPcKeyboard(game, padEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_Y, synthetic, -1)));
        InputDevice keys = gamepad();
        ReflectionHelpers.setField(keys, "mSources", InputDevice.SOURCE_KEYBOARD);
        assertFalse(takenByPcKeyboard(game, padEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_Y, keys, PAD_ID)));
        assertTrue(connection.keys.isEmpty());
        assertSame(focused, pcKeyboardController(game).focus().focused());
        assertEquals(0, heldPadKeys(game).size());
        assertTrue(takenByPcKeyboard(game, padEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_Y, gamepad(), PAD_ID)));
    }

    @Test
    public void aPressWhileClosedDropsAStaleEntryForTheSameKey() {
        Game game = pcKeyboardGame(new RecordingConnection());
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        assertTrue(padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_Y));
        assertTrue(heldPadKeys(game).get(KeyEvent.KEYCODE_BUTTON_Y));
        // The keyboard closes and the pad never reports the release, then Y is pressed for the game.
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        assertFalse(takenByPcKeyboard(game, padEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_Y, gamepad(), PAD_ID)));
        assertEquals(0, heldPadKeys(game).size());
        assertFalse("The release of the game's own press belongs to the game",
                takenByPcKeyboard(game, padEvent(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_Y, gamepad(), PAD_ID)));
    }

    @Test
    public void releasingKeyboardInputAndUnpluggingThePadForgetHeldButtons() {
        RecordingConnection connection = new RecordingConnection();
        Game game = pcKeyboardGame(connection);
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        assertTrue(padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_Y));
        assertEquals(1, heldPadKeys(game).size());
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "releaseKeyboardInput");
        assertEquals(0, heldPadKeys(game).size());

        // Space is held by the pad when it is unplugged: the key goes up and the entry goes away.
        connection.keys.clear();
        assertTrue(padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_Y));
        assertEquals(KeyboardPacket.KEY_DOWN, lastKey(connection)[1]);
        InputManager.InputDeviceListener listener = ReflectionHelpers.getField(game, "pcKeyboardDeviceListener");
        listener.onInputDeviceRemoved(PAD_ID);
        assertEquals(0, heldPadKeys(game).size());
        assertEquals(KeyboardPacket.KEY_UP, lastKey(connection)[1]);
        assertEquals(0x8000 | 0x20, lastKey(connection)[0]);
    }

    @Test
    public void theUsersButtonMapReplacesFaceButtonFlippingOnTheKeyboard() {
        RecordingConnection connection = new RecordingConnection();
        Game game = pcKeyboardGame(connection);
        ControllerHandler handler = ReflectionHelpers.getField(game, "controllerHandler");
        Map<Integer, Object> contexts = ReflectionHelpers.getField(handler, "inputDeviceContexts");
        Object context = ReflectionHelpers.getField(handler, "defaultContext");
        ControllerButtonMap map = new ControllerButtonMap();
        map.put(KeyEvent.KEYCODE_BUTTON_X, KeyEvent.KEYCODE_BUTTON_Y);
        ReflectionHelpers.setField(context, "buttonMap", map);
        contexts.put(PAD_ID, context);
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        connection.keys.clear();
        // X was remapped to Y, which is Space.
        assertTrue(padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_X));
        assertEquals(0x8000 | 0x20, lastKey(connection)[0]);
        assertTrue(padKey(game, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_X));
        // A disabled button does nothing but is still the keyboard's.
        map.put(KeyEvent.KEYCODE_BUTTON_Y, ControllerButtonMap.DISABLED);
        connection.keys.clear();
        assertTrue(padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_Y));
        assertTrue(padKey(game, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_Y));
        assertTrue(connection.keys.isEmpty());
    }

    @Test
    public void menuIsStartAndEnterOnTheKeyboard() {
        RecordingConnection connection = new RecordingConnection();
        Game game = pcKeyboardGame(connection);
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        connection.keys.clear();
        assertTrue(takenByPcKeyboard(game, padEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_MENU, gamepad(), PAD_ID)));
        assertTrue(takenByPcKeyboard(game, padEvent(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_MENU, gamepad(), PAD_ID)));
        assertEquals(2, connection.keys.size());
        assertEquals(0x8000 | 0x0D, connection.keys.get(0)[0]);
    }

    @Test
    public void switchingLayoutResyncsTheUsbPadSide() {
        Game game = pcKeyboardGame(new RecordingConnection());
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        ControllerHandler handler = ReflectionHelpers.getField(game, "controllerHandler");
        java.util.concurrent.atomic.AtomicInteger generation = ReflectionHelpers.getField(handler, "captureGeneration");
        int before = generation.get();
        PcKeyboardController keyboard = pcKeyboardController(game);
        keyboard.setLayout(PcKeyboardLayout.byId(PcKeyboardLayout.DE));
        assertTrue(generation.get() > before);
    }

    @Test
    public void losingFocusLetsGoOfAHeldKey() {
        RecordingConnection connection = new RecordingConnection();
        Game game = pcKeyboardGame(connection);
        ReflectionHelpers.setField(game, "inputCaptureProvider", new Capture());
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        PcKeyboardController keyboard = pcKeyboardController(game);
        keyboard.touchDown(keyboard.layout().key("A"), 1, 1000);
        assertEquals(KeyboardPacket.KEY_DOWN, lastKey(connection)[1]);
        ReflectionHelpers.callInstanceMethod(game.touchInput, "setInputGrabState",
                ReflectionHelpers.ClassParameter.from(boolean.class, false));
        assertEquals(KeyboardPacket.KEY_UP, lastKey(connection)[1]);
        Map<Short, Byte> pressed = ReflectionHelpers.getField(game.keyboardInput, "pressedKeys");
        assertTrue(pressed.isEmpty());
    }

    @Test
    public void losingFocusLetsGoOfALockedModifier() {
        RecordingConnection connection = new RecordingConnection();
        Game game = pcKeyboardGame(connection);
        ReflectionHelpers.setField(game, "inputCaptureProvider", new Capture());
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        PcKeyboardController keyboard = pcKeyboardController(game);
        keyboard.engine().tapModifier(PcModifier.CTRL);
        keyboard.engine().tapModifier(PcModifier.CTRL);
        keyboard.touchDown(keyboard.layout().key("A"), 1, 1000);
        keyboard.touchUp(1);
        connection.keys.clear();
        // Ctrl is still down on the host.
        Map<Short, Byte> pressed = ReflectionHelpers.getField(game.keyboardInput, "pressedKeys");
        assertFalse(pressed.isEmpty());
        ReflectionHelpers.callInstanceMethod(game.touchInput, "setInputGrabState",
                ReflectionHelpers.ClassParameter.from(boolean.class, false));
        assertTrue(pressed.isEmpty());
        assertFalse(connection.keys.isEmpty());
        for (int[] key : connection.keys) assertEquals(KeyboardPacket.KEY_UP, key[1]);
    }

    @Test
    public void dpadMovesFocusAndAPressesTheFocusedKey() {
        RecordingConnection connection = new RecordingConnection();
        Game game = pcKeyboardGame(connection);
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "togglePcKeyboard");
        PcKeyboardController keyboard = pcKeyboardController(game);
        PcKey start = keyboard.focus().focused();
        connection.keys.clear();
        assertTrue(padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_RIGHT));
        assertTrue(padKey(game, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_DPAD_RIGHT));
        assertNotSame(start, keyboard.focus().focused());
        assertTrue("Moving focus presses nothing", connection.keys.isEmpty());
        assertTrue(padKey(game, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_CENTER));
        assertTrue(padKey(game, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_DPAD_CENTER));
        assertEquals(2, connection.keys.size());
        assertEquals(0x8000 | keyboard.focus().focused().vk, connection.keys.get(0)[0]);
    }

    private static class Capture extends InputCaptureProvider {
        boolean visible() { return isCursorVisible; }
    }

    static Game game(RecordingConnection connection) {
        Game game = Robolectric.buildActivity(Game.class).get();
        ReflectionHelpers.setField(game, "conn", connection);
        ReflectionHelpers.setField(game, "keyboardTranslator", new KeyboardTranslator());
        // Grabbing input reads Send system keys
        ReflectionHelpers.setField(game, "prefConfig", new PreferenceConfiguration());
        ReflectionHelpers.setField(game, "grabbedInput", true);
        ReflectionHelpers.setField(game, "foreground", true);
        ReflectionHelpers.setField(game, "connected", true);
        return game;
    }

    /** A key of a physical keyboard (a device that is not a game controller). */
    private static KeyEvent keyboardKey(int action, int keyCode) {
        InputDevice device = ShadowInputDevice.makeInputDeviceNamed("Keyboard");
        ReflectionHelpers.setField(device, "mId", 31);
        ReflectionHelpers.setField(device, "mSources", InputDevice.SOURCE_KEYBOARD);
        ReflectionHelpers.setField(device, "mKeyboardType", InputDevice.KEYBOARD_TYPE_ALPHABETIC);
        ReflectionHelpers.setField(device, "mMotionRanges", new ArrayList<>());
        KeyEvent event = new KeyEvent(0, 0, action, keyCode, 0, 0, 31, 0, 0, InputDevice.SOURCE_KEYBOARD);
        shadowOf(event).setDevice(device);
        return event;
    }

    private static void sendShortcut(Game game, int... keys) {
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "sendKeyboardShortcut",
                ReflectionHelpers.ClassParameter.from(int[].class, keys));
    }

    static class RecordingConnection extends NvConnection {
        final List<int[]> keys = new ArrayList<>();
        final List<String> text = new ArrayList<>();
        final List<Integer> controllerButtons = new ArrayList<>();
        boolean inputAllowed = true;

        RecordingConnection() {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }

        @Override public boolean canSendInput() { return inputAllowed; }
        @Override public void sendUtf8Text(String value) { text.add(value); }
        @Override public void sendControllerInput(short controllerNumber, short activeGamepadMask, int buttonFlags,
                byte leftTrigger, byte rightTrigger, short leftStickX, short leftStickY,
                short rightStickX, short rightStickY) {
            controllerButtons.add(buttonFlags);
        }
        @Override public void sendKeyboardInput(short key, byte direction, byte modifiers, byte flags) {
            keys.add(new int[] {key & 0xffff, direction, modifiers});
        }
    }
}

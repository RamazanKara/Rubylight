package com.limelight;

import android.app.Application;
import android.content.ComponentName;
import android.content.Context;
import android.graphics.Insets;
import android.view.KeyEvent;
import android.view.View;
import android.view.WindowInsets;
import android.view.WindowManager;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputMethodManager;
import android.widget.FrameLayout;

import com.limelight.binding.input.KeyboardTranslator;
import com.limelight.binding.input.capture.InputCaptureProvider;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.input.KeyboardPacket;
import com.limelight.ui.StreamView;
import com.limelight.ui.StreamMenu;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
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
        ReflectionHelpers.callInstanceMethod(game, "toggleKeyboardOverlay");
        View panel = ReflectionHelpers.getField(game, "keyboardOverlay");
        assertEquals(View.VISIBLE, panel.getVisibility());
        InputMethodManager input = (InputMethodManager) game.getSystemService(Context.INPUT_METHOD_SERVICE);
        assertFalse(shadowOf(input).isSoftInputVisible());

        ReflectionHelpers.callInstanceMethod(game, "onStreamMenuItem",
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
        ReflectionHelpers.callInstanceMethod(game, "toggleKeyboardOverlay");
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
        Map<Short, Byte> pressed = ReflectionHelpers.getField(game, "pressedKeys");
        assertTrue(pressed.isEmpty());
    }

    @Test
    public void shortcutsPreservePhysicalModifiersAndRespectInputGrab() {
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        short ctrl = new KeyboardTranslator().translate(KeyEvent.KEYCODE_CTRL_LEFT, -1);
        Map<Short, Byte> pressed = ReflectionHelpers.getField(game, "pressedKeys");
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
            ReflectionHelpers.callInstanceMethod(game, "toggleKeyboardOverlay");
            assertTrue(capture.visible());
            assertTrue((boolean) ReflectionHelpers.getField(game, "grabbedInput"));
            ReflectionHelpers.callInstanceMethod(game, "setInputGrabState",
                    ReflectionHelpers.ClassParameter.from(boolean.class, true));
            assertTrue(capture.visible());
            ReflectionHelpers.callInstanceMethod(game, "hideKeyboardOverlay");
            assertEquals(visible, capture.visible());
            assertTrue((boolean) ReflectionHelpers.getField(game, "grabbedInput"));
        }
    }

    private static class Capture extends InputCaptureProvider {
        boolean visible() { return isCursorVisible; }
    }

    private static Game game(RecordingConnection connection) {
        Game game = Robolectric.buildActivity(Game.class).get();
        ReflectionHelpers.setField(game, "conn", connection);
        ReflectionHelpers.setField(game, "keyboardTranslator", new KeyboardTranslator());
        ReflectionHelpers.setField(game, "grabbedInput", true);
        ReflectionHelpers.setField(game, "foreground", true);
        ReflectionHelpers.setField(game, "connected", true);
        return game;
    }

    private static void sendShortcut(Game game, int... keys) {
        ReflectionHelpers.callInstanceMethod(game, "sendKeyboardShortcut",
                ReflectionHelpers.ClassParameter.from(int[].class, keys));
    }

    private static class RecordingConnection extends NvConnection {
        final List<int[]> keys = new ArrayList<>();
        final List<String> text = new ArrayList<>();
        boolean inputAllowed = true;

        RecordingConnection() {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }

        @Override public boolean canSendInput() { return inputAllowed; }
        @Override public void sendUtf8Text(String value) { text.add(value); }
        @Override public void sendKeyboardInput(short key, byte direction, byte modifiers, byte flags) {
            keys.add(new int[] {key & 0xffff, direction, modifiers});
        }
    }
}

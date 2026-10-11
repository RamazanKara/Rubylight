package com.limelight;

import android.app.Application;
import android.view.KeyEvent;
import android.widget.FrameLayout;

import com.limelight.binding.input.KeyboardTranslator;
import com.limelight.binding.input.capture.InputCaptureProvider;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.input.KeyboardPacket;
import com.limelight.ui.StreamView;

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

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameKeyboardOverlayTest {
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
        boolean inputAllowed = true;

        RecordingConnection() {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }

        @Override public boolean canSendInput() { return inputAllowed; }
        @Override public void sendKeyboardInput(short key, byte direction, byte modifiers, byte flags) {
            keys.add(new int[] {key & 0xffff, direction, modifiers});
        }
    }
}

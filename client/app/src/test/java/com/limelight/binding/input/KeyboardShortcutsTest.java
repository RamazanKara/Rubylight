package com.limelight.binding.input;

import android.app.Application;
import android.view.KeyEvent;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.util.Arrays;
import java.util.List;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class KeyboardShortcutsTest {
    @Test public void chordsAndExportsRoundTripWithoutExposingMutableKeys() {
        int[] keys = KeyboardShortcuts.parseChord("Ctrl + Shift + Esc");
        assertArrayEquals(new int[] {KeyEvent.KEYCODE_CTRL_LEFT, KeyEvent.KEYCODE_SHIFT_LEFT, KeyEvent.KEYCODE_ESCAPE}, keys);
        KeyboardShortcuts.Shortcut shortcut = new KeyboardShortcuts.Shortcut("task-manager", "Task Manager", keys);
        keys[0] = KeyEvent.KEYCODE_A;
        List<KeyboardShortcuts.Shortcut> entries = KeyboardShortcuts.parse(KeyboardShortcuts.encode(Arrays.asList(shortcut)));
        assertEquals("Task Manager", entries.get(0).name);
        assertEquals(KeyEvent.KEYCODE_CTRL_LEFT, entries.get(0).keys()[0]);
        assertArrayEquals(shortcut.keys(), KeyboardShortcuts.parseChord(KeyboardShortcuts.format(shortcut.keys())));
        assertArrayEquals(new int[] {KeyEvent.KEYCODE_MOVE_HOME, KeyEvent.KEYCODE_MOVE_END}, KeyboardShortcuts.parseChord("HOME + END"));
    }

    @Test public void importsCommonArtemisVirtualKeys() {
        List<KeyboardShortcuts.Shortcut> entries = KeyboardShortcuts.parse(
                "{\"data\":[{\"id\":\"copy\",\"name\":\"Copy\",\"keys\":[\"VK_LCONTROL\",\"VK_C\"]}]}");
        assertArrayEquals(new int[] {KeyEvent.KEYCODE_CTRL_LEFT, KeyEvent.KEYCODE_C}, entries.get(0).keys());
        assertArrayEquals(new int[] {KeyEvent.KEYCODE_A}, KeyboardShortcuts.parseChord("0x41"));
    }

    @Test public void everySupportedKeySurvivesExportAndEditorRoundTrips() {
        for (int key = 1; key <= Math.max(KeyEvent.getMaxKeyCode(), KeyEvent.KEYCODE_F24); key++) {
            if (KeyboardTranslator.translateKeyCode(key) == 0) continue;
            KeyboardShortcuts.Shortcut shortcut = new KeyboardShortcuts.Shortcut("key-" + key, "Key " + key, new int[] {key});
            List<KeyboardShortcuts.Shortcut> imported = KeyboardShortcuts.parse(KeyboardShortcuts.encode(Arrays.asList(shortcut)));
            assertArrayEquals("Export key " + key, shortcut.keys(), imported.get(0).keys());
            assertArrayEquals("Editor key " + key, shortcut.keys(), KeyboardShortcuts.parseChord(KeyboardShortcuts.format(shortcut.keys())));
        }
        assertArrayEquals(new int[] {KeyEvent.KEYCODE_7}, KeyboardShortcuts.parseChord("7"));
        assertArrayEquals(new int[] {KeyEvent.KEYCODE_A}, KeyboardShortcuts.parseChord("ANDROID_KEYCODE_" + KeyEvent.KEYCODE_A));
    }

    @Test public void rejectsInvalidImportsAndOversizedFiles() {
        for (String chord : new String[] {"", "CTRL+", "CTRL+CTRL", "POWER", "UNKNOWN", "A+B+C+D+E+F+G+H+I"}) {
            assertThrows(IllegalArgumentException.class, () -> KeyboardShortcuts.parseChord(chord));
        }
        String item = "{\"id\":\"same\",\"name\":\"Copy\",\"keys\":[\"CTRL\",\"C\"]}";
        assertThrows(IllegalArgumentException.class, () -> KeyboardShortcuts.parse("{\"data\":[" + item + "," + item + "]}"));
        assertThrows(IllegalArgumentException.class, () -> KeyboardShortcuts.parse("{\"version\":2,\"data\":[]}"));
        assertThrows(IOException.class, () -> KeyboardShortcuts.read(new ByteArrayInputStream(new byte[KeyboardShortcuts.MAX_BYTES + 1])));
    }
}

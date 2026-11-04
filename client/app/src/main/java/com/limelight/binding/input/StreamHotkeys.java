package com.limelight.binding.input;

import android.content.SharedPreferences;
import android.view.KeyEvent;

import com.limelight.nvstream.input.KeyboardPacket;
import com.limelight.ui.StreamMenuModel;
import com.limelight.ui.StreamMenuModel.Chord;
import com.limelight.ui.StreamMenuModel.Hotkey;

import java.util.Collections;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;

/**
 * The stream hotkeys (Ctrl+Alt+Shift+Tab opens the stream menu, …) from the table desktop Nightjar shares
 * (protocol/policy shortcuts.rs), with the user's changes saved as {@code stream_hotkey_<id>}: the keys joined by
 * {@code +}, or {@code off}. An action without a saved value has its default.
 */
public final class StreamHotkeys {
    public static final String PREFIX = "stream_hotkey_";
    public static final String OFF = "off";
    /** An old chord's one notice that it moved, per action: {@code stream_hotkeys_moved_seen_<id>}. */
    static final String MOVED_SEEN = "stream_hotkeys_moved_seen_";

    public static final String STREAM_MENU = "stream_menu";
    public static final String QUIT = "quit";
    public static final String CAPTURE = "capture";
    public static final String OVERLAY = "overlay";
    public static final String PC_KEYBOARD = "pc_keyboard";
    public static final String CURSOR = "cursor";

    private StreamHotkeys() { }

    /** The user's changes, by action id. */
    public static Map<String, String> bindings(SharedPreferences prefs) {
        Map<String, String> bindings = new TreeMap<>();
        for (Map.Entry<String, ?> entry : prefs.getAll().entrySet()) {
            if (entry.getKey().startsWith(PREFIX) && entry.getValue() instanceof String) {
                bindings.put(entry.getKey().substring(PREFIX.length()), (String) entry.getValue());
            }
        }
        return bindings;
    }

    public static List<Hotkey> load(SharedPreferences prefs) throws StreamMenuModel.Refused {
        return StreamMenuModel.hotkeys(bindings(prefs));
    }

    /** Saves {@code keys} for the action, or throws why the model refused them. */
    public static void record(SharedPreferences prefs, String id, String[] keys) throws StreamMenuModel.Refused {
        String[] saved = StreamMenuModel.checkHotkey(id, keys, bindings(prefs));
        prefs.edit().putString(PREFIX + id, String.join("+", saved)).apply();
    }

    public static void turnOff(SharedPreferences prefs, String id) {
        prefs.edit().putString(PREFIX + id, OFF).apply();
    }

    public static void reset(SharedPreferences prefs, String id) {
        prefs.edit().remove(PREFIX + id).apply();
    }

    /** A pressed key's name as the model saves it; the left and right modifiers are one name. */
    public static String keyName(int keyCode) {
        switch (keyCode) {
            case KeyEvent.KEYCODE_CTRL_LEFT: case KeyEvent.KEYCODE_CTRL_RIGHT: return "CTRL";
            case KeyEvent.KEYCODE_ALT_LEFT: case KeyEvent.KEYCODE_ALT_RIGHT: return "ALT";
            case KeyEvent.KEYCODE_SHIFT_LEFT: case KeyEvent.KEYCODE_SHIFT_RIGHT: return "SHIFT";
            case KeyEvent.KEYCODE_META_LEFT: case KeyEvent.KEYCODE_META_RIGHT: return "WIN";
            default: return KeyboardShortcuts.keyName(keyCode);
        }
    }

    public static boolean isModifier(int keyCode) {
        switch (keyCode) {
            case KeyEvent.KEYCODE_CTRL_LEFT: case KeyEvent.KEYCODE_CTRL_RIGHT:
            case KeyEvent.KEYCODE_ALT_LEFT: case KeyEvent.KEYCODE_ALT_RIGHT:
            case KeyEvent.KEYCODE_SHIFT_LEFT: case KeyEvent.KEYCODE_SHIFT_RIGHT:
            case KeyEvent.KEYCODE_META_LEFT: case KeyEvent.KEYCODE_META_RIGHT:
                return true;
            default:
                return false;
        }
    }

    /** A hotkey that fired, and the old chord that fired it (null for its own chord). */
    public static final class Match {
        public final Hotkey hotkey;
        public final Chord old;

        Match(Hotkey hotkey, Chord old) {
            this.hotkey = hotkey;
            this.old = old;
        }

        /** Whether this is the first time an old chord fired it: that one time, the stream says where it moved. */
        public boolean firstOldUse(SharedPreferences prefs) {
            if (old == null || prefs.getBoolean(MOVED_SEEN + hotkey.id, false)) return false;
            prefs.edit().putBoolean(MOVED_SEEN + hotkey.id, true).apply();
            return true;
        }
    }

    /** The hotkeys of one stream, matched as the PC would see the keys: the exact modifiers and the Windows VK. */
    public static final class Table {
        private final List<Hotkey> hotkeys;

        public Table(List<Hotkey> hotkeys) {
            this.hotkeys = Collections.unmodifiableList(hotkeys);
        }

        public static final Table EMPTY = new Table(Collections.emptyList());

        /** {@code modifiers} are the held {@link KeyboardPacket} modifier bits, {@code vk} the key's Windows VK. */
        public Match at(int modifiers, int vk) {
            int held = modifiers & (KeyboardPacket.MODIFIER_SHIFT | KeyboardPacket.MODIFIER_CTRL
                    | KeyboardPacket.MODIFIER_ALT | KeyboardPacket.MODIFIER_META);
            for (Hotkey hotkey : hotkeys) {
                if (hotkey.chord != null && hotkey.chord.modifiers == held && hotkey.chord.vk == vk) return new Match(hotkey, null);
            }
            for (Hotkey hotkey : hotkeys) {
                for (Chord old : hotkey.old) if (old.modifiers == held && old.vk == vk) return new Match(hotkey, old);
            }
            return null;
        }
    }
}

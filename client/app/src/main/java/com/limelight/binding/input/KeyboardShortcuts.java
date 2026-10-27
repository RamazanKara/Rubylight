package com.limelight.binding.input;

import android.content.Context;
import android.preference.PreferenceManager;
import android.view.KeyEvent;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Locale;
import java.util.Set;

/** Bounded, portable keyboard chords. Events use the existing keyboard permission and release path. */
public final class KeyboardShortcuts {
    public static final int MAX_SHORTCUTS = 32;
    public static final int MAX_KEYS = 8;
    public static final int MAX_BYTES = 64 * 1024;
    private static final String PREF = "keyboard_shortcuts";

    private KeyboardShortcuts() { }

    public static final class Shortcut {
        public final String id;
        public final String name;
        private final int[] keys;

        public Shortcut(String id, String name, int[] keys) {
            if (id == null || !id.matches("[A-Za-z0-9_-]{1,64}") || name == null ||
                    name.trim().isEmpty() || name.length() > 80 || hasControlCharacter(name) ||
                    keys == null || keys.length == 0 || keys.length > MAX_KEYS) {
                throw new IllegalArgumentException("Invalid shortcut");
            }
            Set<Integer> unique = new HashSet<>();
            for (int key : keys) {
                if (key < 0 || KeyboardTranslator.translateKeyCode(key) == 0 || !unique.add(key)) {
                    throw new IllegalArgumentException("Unsupported or repeated key");
                }
            }
            this.id = id;
            this.name = name.trim();
            this.keys = keys.clone();
        }

        public int[] keys() { return keys.clone(); }
    }

    public static List<Shortcut> load(Context context) {
        try {
            return parse(PreferenceManager.getDefaultSharedPreferences(context).getString(PREF, "{\"data\":[]}"));
        } catch (IllegalArgumentException e) {
            return new ArrayList<>();
        }
    }

    private static boolean hasControlCharacter(String value) {
        for (int i = 0; i < value.length(); i++) {
            if (Character.isISOControl(value.charAt(i))) return true;
        }
        return false;
    }

    public static void save(Context context, List<Shortcut> entries) {
        PreferenceManager.getDefaultSharedPreferences(context).edit().putString(PREF, encode(entries)).apply();
    }

    public static List<Shortcut> read(InputStream input) throws IOException {
        if (input == null) throw new IOException("Cannot open shortcuts");
        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        byte[] buffer = new byte[4096];
        int count;
        while ((count = input.read(buffer, 0, Math.min(buffer.length, MAX_BYTES + 1 - bytes.size()))) != -1) {
            bytes.write(buffer, 0, count);
            if (bytes.size() > MAX_BYTES) throw new IOException("Shortcut file is too large");
        }
        return parse(new String(bytes.toByteArray(), StandardCharsets.UTF_8));
    }

    public static List<Shortcut> parse(String json) {
        if (json == null || json.length() > MAX_BYTES) throw new IllegalArgumentException("Shortcut file is too large");
        try {
            JSONObject document = new JSONObject(json);
            if (document.has("version") && document.getInt("version") != 1) {
                throw new IllegalArgumentException("Unsupported shortcut version");
            }
            JSONArray data = document.getJSONArray("data");
            if (data.length() > MAX_SHORTCUTS) throw new IllegalArgumentException("Too many shortcuts");
            List<Shortcut> entries = new ArrayList<>();
            Set<String> ids = new HashSet<>();
            for (int i = 0; i < data.length(); i++) {
                JSONObject item = data.getJSONObject(i);
                JSONArray codes = item.getJSONArray("keys");
                if (codes.length() == 0 || codes.length() > MAX_KEYS) throw new IllegalArgumentException("Invalid chord length");
                int[] keys = new int[codes.length()];
                for (int k = 0; k < keys.length; k++) keys[k] = parseKey(codes.getString(k));
                Shortcut entry = new Shortcut(item.getString("id"), item.getString("name"), keys);
                if (!ids.add(entry.id)) throw new IllegalArgumentException("Repeated shortcut id");
                entries.add(entry);
            }
            return entries;
        } catch (JSONException e) {
            throw new IllegalArgumentException("Invalid shortcut file", e);
        }
    }

    public static String encode(List<Shortcut> entries) {
        if (entries.size() > MAX_SHORTCUTS) throw new IllegalArgumentException("Too many shortcuts");
        try {
            JSONArray data = new JSONArray();
            Set<String> ids = new HashSet<>();
            for (Shortcut entry : entries) {
                if (!ids.add(entry.id)) throw new IllegalArgumentException("Repeated shortcut id");
                JSONArray keys = new JSONArray();
                for (int key : entry.keys) keys.put(keyName(key));
                data.put(new JSONObject().put("id", entry.id).put("name", entry.name).put("keys", keys));
            }
            return new JSONObject().put("version", 1).put("data", data).toString(2);
        } catch (JSONException e) {
            throw new IllegalArgumentException("Invalid shortcut", e);
        }
    }

    public static int[] parseChord(String chord) {
        if (chord == null || chord.length() > 256) throw new IllegalArgumentException("Invalid chord");
        String[] names = chord.split("\\+", -1);
        if (names.length == 0 || names.length > MAX_KEYS) throw new IllegalArgumentException("Invalid chord length");
        int[] keys = new int[names.length];
        for (int i = 0; i < names.length; i++) keys[i] = parseKey(names[i]);
        // Reuse duplicate and supported-key validation for editor input.
        return new Shortcut("validation", "validation", keys).keys();
    }

    public static String format(int[] keys) {
        StringBuilder value = new StringBuilder();
        for (int key : keys) {
            if (value.length() != 0) value.append(" + ");
            value.append(keyName(key));
        }
        return value.toString();
    }

    // F13-F24 are inlined numeric constants translated by our own table, including on API 21.
    @android.annotation.SuppressLint("InlinedApi")
    private static String keyName(int key) {
        switch (key) {
            case KeyEvent.KEYCODE_CTRL_LEFT: return "CTRL";
            case KeyEvent.KEYCODE_SHIFT_LEFT: return "SHIFT";
            case KeyEvent.KEYCODE_ALT_LEFT: return "ALT";
            case KeyEvent.KEYCODE_META_LEFT: return "WIN";
            case KeyEvent.KEYCODE_ESCAPE: return "ESC";
            case KeyEvent.KEYCODE_DEL: return "BACKSPACE";
            case KeyEvent.KEYCODE_FORWARD_DEL: return "DELETE";
            case KeyEvent.KEYCODE_MOVE_HOME: return "HOME";
            case KeyEvent.KEYCODE_MOVE_END: return "END";
            default:
                if (key >= KeyEvent.KEYCODE_A && key <= KeyEvent.KEYCODE_Z) return String.valueOf((char) ('A' + key - KeyEvent.KEYCODE_A));
                if (key >= KeyEvent.KEYCODE_0 && key <= KeyEvent.KEYCODE_9) return String.valueOf(key - KeyEvent.KEYCODE_0);
                if (key >= KeyEvent.KEYCODE_F1 && key <= KeyEvent.KEYCODE_F12) return "F" + (key - KeyEvent.KEYCODE_F1 + 1);
                if (key >= KeyEvent.KEYCODE_F13 && key <= KeyEvent.KEYCODE_F24) return "F" + (key - KeyEvent.KEYCODE_F13 + 13);
                if (key >= KeyEvent.KEYCODE_NUMPAD_0 && key <= KeyEvent.KEYCODE_NUMPAD_9) return "NUMPAD_" + (key - KeyEvent.KEYCODE_NUMPAD_0);
                String symbolic = KeyEvent.keyCodeToString(key);
                // Older platforms may only expose a numeric code. Keep it explicitly tagged
                // so it can round-trip without confusing Android code 7 with the digit 7 key.
                return symbolic.startsWith("KEYCODE_") ? symbolic.substring(8) : "ANDROID_KEYCODE_" + key;
        }
    }

    @android.annotation.SuppressLint("InlinedApi")
    private static int parseKey(String value) {
        String name = value.trim().toUpperCase(Locale.ROOT);
        boolean virtual = name.startsWith("VK_");
        if (virtual) name = name.substring(3);
        if (virtual && name.equals("MENU")) name = "ALT";
        switch (name) {
            case "CTRL": case "CONTROL": case "LCONTROL": name = "CTRL_LEFT"; break;
            case "RCONTROL": name = "CTRL_RIGHT"; break;
            case "SHIFT": case "LSHIFT": name = "SHIFT_LEFT"; break;
            case "RSHIFT": name = "SHIFT_RIGHT"; break;
            case "ALT": case "LMENU": name = "ALT_LEFT"; break;
            case "RMENU": name = "ALT_RIGHT"; break;
            case "WIN": case "LWIN": name = "META_LEFT"; break;
            case "RWIN": name = "META_RIGHT"; break;
            case "ESC": name = "ESCAPE"; break;
            case "BACKSPACE": case "BACK": name = "DEL"; break;
            case "DELETE": name = "FORWARD_DEL"; break;
            case "RETURN": name = "ENTER"; break;
            case "HOME": name = "MOVE_HOME"; break;
            case "END": name = "MOVE_END"; break;
            case "CAPITAL": name = "CAPS_LOCK"; break;
            case "NUMLOCK": name = "NUM_LOCK"; break;
            case "SCROLL": name = "SCROLL_LOCK"; break;
            case "SNAPSHOT": name = "SYSRQ"; break;
            case "PAUSE": name = "BREAK"; break;
            case "PRIOR": name = "PAGE_UP"; break;
            case "NEXT": name = "PAGE_DOWN"; break;
            case "LEFT": name = "DPAD_LEFT"; break;
            case "RIGHT": name = "DPAD_RIGHT"; break;
            case "UP": name = "DPAD_UP"; break;
            case "DOWN": name = "DPAD_DOWN"; break;
            default: break;
        }
        int key;
        if (name.matches("ANDROID_KEYCODE_[0-9]{1,5}")) key = Integer.parseInt(name.substring(16));
        else if (name.matches("F(?:1[3-9]|2[0-4])")) key = KeyEvent.KEYCODE_F13 + Integer.parseInt(name.substring(1)) - 13;
        else if (name.matches("0X[0-9A-F]{1,2}")) {
            int vk = Integer.parseInt(name.substring(2), 16);
            key = KeyEvent.KEYCODE_UNKNOWN;
            for (int candidate = 1; candidate <= Math.max(KeyEvent.getMaxKeyCode(), KeyEvent.KEYCODE_F24); candidate++) {
                if ((KeyboardTranslator.translateKeyCode(candidate) & 0xffff) == (0x8000 | vk)) {
                    key = candidate;
                    break;
                }
            }
        } else key = KeyEvent.keyCodeFromString(name.startsWith("KEYCODE_") ? name : "KEYCODE_" + name);
        if (key == KeyEvent.KEYCODE_UNKNOWN || KeyboardTranslator.translateKeyCode(key) == 0) {
            throw new IllegalArgumentException("Unsupported key: " + value);
        }
        return key;
    }
}

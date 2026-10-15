package com.limelight.binding.input;

import android.view.KeyEvent;
import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;
import org.json.JSONTokener;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Iterator;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Set;

/**
 * Reads a Nightjar layout, or Artemis exports: the on-screen keyboard layout ("Export keyboard
 * layout", element ID to a JSON string with pixel LEFT, TOP, WIDTH, HEIGHT, ENABLED and HIDDEN) and the
 * Custom Keys file ({"data":[{"id","name","sticky","keys":["VK_*" or "0x.."]}]}). Artemis stores pixels
 * without the screen size, so positions are read against the surface they are imported on.
 */
public final class TouchLayoutImport {
    public enum Format { NIGHTJAR, ARTEMIS_LAYOUT, ARTEMIS_CUSTOM_KEYS }

    public enum Skip {
        /** Artemis touchpad areas and any element Nightjar has no control for. */
        NO_EQUIVALENT,
        /** A key the Moonlight protocol cannot send, or a chord mixing keys and mouse buttons. */
        UNSUPPORTED_KEY,
        /** A custom key placed in the layout whose Custom Keys file was not imported with it. */
        MISSING_CUSTOM_KEY,
        /** The double-tap key in the center of an Artemis keyboard stick. */
        STICK_CENTER_KEY,
        /** Past the 32 controls a layout holds. */
        TOO_MANY,
        /** Zero or negative size. */
        INVALID_SIZE
    }

    public static final class Skipped {
        public final String name;
        public final Skip reason;
        Skipped(String name, Skip reason) { this.name = name; this.reason = reason; }
    }

    public static final class Result {
        public final TouchLayout layout;
        public final Format format;
        public final List<Skipped> skipped;
        /** Elements that were turned off or hidden in Artemis and so were not imported. */
        public final int hidden;
        /** The Artemis layout reached past this surface and was scaled down to fit. */
        public final boolean scaled;
        Result(TouchLayout layout, Format format, List<Skipped> skipped, int hidden, boolean scaled) {
            this.layout = layout; this.format = format; this.skipped = Collections.unmodifiableList(skipped);
            this.hidden = hidden; this.scaled = scaled;
        }
    }

    // The keys of Artemis's built-in stick and D-pad (assets/config/keyboard.json): W, A, S, D.
    private static final int[] ARTEMIS_WASD = {KeyEvent.KEYCODE_W, KeyEvent.KEYCODE_A, KeyEvent.KEYCODE_S, KeyEvent.KEYCODE_D};
    // Artemis lays out custom keys on a 72-unit grid of the screen height, 10 units per key.
    private static final float ARTEMIS_KEY = 10 / 72f;

    private TouchLayoutImport() { }

    /**
     * Imports one Nightjar file, or an Artemis layout and/or Custom Keys file. A Nightjar layout is
     * already relative to the surface; Artemis files need the surface's size in pixels.
     */
    public static Result importFiles(List<String> files, int surfaceWidth, int surfaceHeight) {
        if (files.isEmpty()) throw notLayout(null);
        JSONObject layout = null, customKeys = null;
        for (String text : files) {
            if (text == null || text.getBytes(java.nio.charset.StandardCharsets.UTF_8).length > TouchLayout.MAX_BYTES) {
                throw new TouchLayout.InvalidLayoutException(TouchLayout.InvalidLayoutException.Reason.TOO_LARGE, 0, null);
            }
            Format format = detect(text);
            if (format == Format.NIGHTJAR) {
                if (files.size() != 1) throw notLayout("A Nightjar layout is imported on its own");
                return new Result(TouchLayout.parse(text), Format.NIGHTJAR, new ArrayList<>(), 0, false);
            }
            JSONObject document = object(text);
            if (format == Format.ARTEMIS_LAYOUT && layout == null) layout = document;
            else if (format == Format.ARTEMIS_CUSTOM_KEYS && customKeys == null) customKeys = document;
            else throw notLayout(format == null ? null : "Two files of the same kind");
        }
        if (surfaceWidth <= 0 || surfaceHeight <= 0) throw notLayout("No surface size to place Artemis controls on");
        Map<String, JSONObject> keys = customKeys == null ? null : customKeys(customKeys);
        List<Skipped> skipped = new ArrayList<>();
        if (layout == null) {
            return new Result(placeCustomKeys(customKeys, keys, surfaceWidth, surfaceHeight, skipped),
                    Format.ARTEMIS_CUSTOM_KEYS, skipped, 0, false);
        }
        return artemisLayout(layout, keys, surfaceWidth, surfaceHeight, skipped);
    }

    /** What kind of file this is, or null when it is none of the supported kinds. */
    public static Format detect(String text) {
        JSONObject document;
        try { document = object(text); }
        catch (TouchLayout.InvalidLayoutException e) { return null; }
        if (document.has("version") && document.has("buttons")) return Format.NIGHTJAR;
        if (document.optJSONArray("data") != null) return Format.ARTEMIS_CUSTOM_KEYS;
        Iterator<String> names = document.keys();
        if (!names.hasNext()) return null;
        while (names.hasNext()) {
            String name = names.next();
            JSONObject element = element(document.opt(name));
            if (element == null) return null;
        }
        return Format.ARTEMIS_LAYOUT;
    }

    private static JSONObject object(String text) {
        try {
            Object value = new JSONTokener(text).nextValue();
            if (value instanceof JSONObject) return (JSONObject) value;
        } catch (JSONException ignored) {
            // Reported below as not a layout.
        }
        throw notLayout(null);
    }

    private static TouchLayout.InvalidLayoutException notLayout(String detail) {
        return new TouchLayout.InvalidLayoutException(TouchLayout.InvalidLayoutException.Reason.NOT_JSON, 0, detail);
    }

    // An exported element is a JSON object serialized into a string by Gson.
    private static JSONObject element(Object value) {
        if (!(value instanceof String)) return null;
        try {
            Object parsed = new JSONTokener((String) value).nextValue();
            if (!(parsed instanceof JSONObject)) return null;
            JSONObject element = (JSONObject) parsed;
            return element.has("LEFT") && element.has("TOP") && element.has("WIDTH") && element.has("HEIGHT") ? element : null;
        } catch (JSONException e) {
            return null;
        }
    }

    private static Map<String, JSONObject> customKeys(JSONObject file) {
        Map<String, JSONObject> result = new HashMap<>();
        JSONArray data = file.optJSONArray("data");
        for (int i = 0; data != null && i < data.length(); i++) {
            JSONObject key = data.optJSONObject(i);
            if (key == null) continue;
            // Artemis falls back to the position in the list for an entry without an ID.
            String id = key.optString("id", "");
            result.put(id.isEmpty() ? Integer.toString(i) : id, key);
        }
        return result;
    }

    private static final class Placed {
        final String elementId;
        final float left, top, width, height;
        Placed(String elementId, JSONObject element) {
            this.elementId = elementId;
            left = (float) element.optDouble("LEFT", 0); top = (float) element.optDouble("TOP", 0);
            width = (float) element.optDouble("WIDTH", 0); height = (float) element.optDouble("HEIGHT", 0);
        }
    }

    private static Result artemisLayout(JSONObject layout, Map<String, JSONObject> customKeys,
                                        int surfaceWidth, int surfaceHeight, List<Skipped> skipped) {
        List<Placed> placed = new ArrayList<>();
        int hidden = 0;
        Iterator<String> names = layout.keys();
        while (names.hasNext()) {
            String id = names.next();
            JSONObject element = element(layout.opt(id));
            if (element == null) continue;
            if (!element.optBoolean("ENABLED", true) || element.optBoolean("HIDDEN", false)) {
                hidden++;
                continue;
            }
            Placed item = new Placed(id, element);
            if (!(item.width > 0 && item.height > 0) || Float.isNaN(item.left) || Float.isNaN(item.top)) {
                skipped.add(new Skipped(id, Skip.INVALID_SIZE));
                continue;
            }
            placed.add(item);
        }
        // Reading order, so the list and TalkBack follow the screen.
        Collections.sort(placed, (a, b) -> a.top != b.top ? Float.compare(a.top, b.top) :
                a.left != b.left ? Float.compare(a.left, b.left) : a.elementId.compareTo(b.elementId));
        float right = 0, bottom = 0;
        for (Placed item : placed) {
            right = Math.max(right, item.left + item.width);
            bottom = Math.max(bottom, item.top + item.height);
        }
        float scale = Math.min(1, Math.min(right > surfaceWidth ? surfaceWidth / right : 1,
                bottom > surfaceHeight ? surfaceHeight / bottom : 1));
        List<TouchLayout.Button> buttons = new ArrayList<>();
        Set<String> ids = new HashSet<>();
        for (Placed item : placed) {
            float x = item.left * scale / surfaceWidth, y = item.top * scale / surfaceHeight;
            float w = item.width * scale / surfaceWidth, h = item.height * scale / surfaceHeight;
            TouchLayout.Button button = artemisElement(item.elementId, customKeys, skipped, ids, x, y, w, h);
            if (button == null) continue;
            if (buttons.size() == TouchLayout.MAX_BUTTONS) skipped.add(new Skipped(button.name, Skip.TOO_MANY));
            else buttons.add(button);
        }
        return new Result(new TouchLayout(buttons), Format.ARTEMIS_LAYOUT, skipped, hidden, scale < 1);
    }

    private static TouchLayout.Button artemisElement(String elementId, Map<String, JSONObject> customKeys,
                                                     List<Skipped> skipped, Set<String> ids,
                                                     float x, float y, float w, float h) {
        // Nightjar keeps every control at least 4% and at most 80% of the surface.
        w = Math.max(.04f, Math.min(.8f, w)); h = Math.max(.04f, Math.min(.8f, h));
        x = Math.max(0, Math.min(1 - w, x)); y = Math.max(0, Math.min(1 - h, y));
        String id = uniqueId(elementId, ids);
        try {
            if (elementId.startsWith("custom_")) {
                JSONObject key = customKeys == null ? null : customKeys.get(elementId.substring(7));
                String name = key == null ? elementId.substring(7) : label(key.optString("name", ""), elementId.substring(7));
                if (key == null) {
                    skipped.add(new Skipped(name, Skip.MISSING_CUSTOM_KEY));
                    return null;
                }
                return customKey(key, id, name, skipped, x, y, w, h);
            }
            if (elementId.startsWith("rocker_") || elementId.startsWith("dpad_")) {
                boolean stick = elementId.startsWith("rocker_");
                // Artemis's stick holds Shift when its center is double-tapped; Nightjar has no such key.
                if (stick) skipped.add(new Skipped(elementId, Skip.STICK_CENTER_KEY));
                return TouchLayout.Button.wasd(id, "WASD", ARTEMIS_WASD, false,
                        stick ? TouchLayout.DEFAULT_DEADZONE : LayoutStick.DPAD_DEADZONE,
                        stick ? TouchLayout.DEFAULT_DIAGONAL : LayoutStick.DPAD_DIAGONAL, x, y, w, h);
            }
            boolean toggle = elementId.startsWith("key_s_") || elementId.startsWith("m_s_");
            if (elementId.startsWith("key_")) {
                int code = Integer.parseInt(elementId.substring(toggle ? 6 : 4));
                return new TouchLayout.Button(id, keyName(code), KeyboardShortcuts.parseChord(chordFor(code)), 0, toggle, x, y, w, h);
            }
            if (elementId.startsWith("m_")) {
                int code = Integer.parseInt(elementId.substring(toggle ? 4 : 2));
                if (code >= 1 && code <= 5) {
                    return new TouchLayout.Button(id, code == 1 ? "ML" : code == 2 ? "MM" : code == 3 ? "MR" : "M" + code,
                            new int[0], code, toggle, x, y, w, h);
                }
            }
        } catch (IllegalArgumentException e) {
            skipped.add(new Skipped(elementId, Skip.UNSUPPORTED_KEY));
            return null;
        }
        skipped.add(new Skipped(elementId, Skip.NO_EQUIVALENT));
        return null;
    }

    // Android key code to a chord string the shortcut parser reads back.
    private static String chordFor(int code) {
        return "ANDROID_KEYCODE_" + code;
    }

    private static String keyName(int code) {
        switch (code) {
            case KeyEvent.KEYCODE_DPAD_UP: return "↑";
            case KeyEvent.KEYCODE_DPAD_DOWN: return "↓";
            case KeyEvent.KEYCODE_DPAD_LEFT: return "←";
            case KeyEvent.KEYCODE_DPAD_RIGHT: return "→";
            case KeyEvent.KEYCODE_DEL: return "⌫";
            default: return KeyboardShortcuts.format(new int[] {code});
        }
    }

    private static TouchLayout.Button customKey(JSONObject key, String id, String name, List<Skipped> skipped,
                                                float x, float y, float w, float h) {
        JSONArray codes = key.optJSONArray("keys");
        boolean sticky = key.optBoolean("sticky", false);
        if (codes == null || codes.length() == 0) {
            skipped.add(new Skipped(name, Skip.UNSUPPORTED_KEY));
            return null;
        }
        StringBuilder chord = new StringBuilder();
        int mouse = 0;
        for (int i = 0; i < codes.length(); i++) {
            int vk = virtualKey(codes.optString(i, ""));
            int button = mouseButton(vk);
            if (vk < 0 || (button != 0 && codes.length() != 1)) {
                skipped.add(new Skipped(name, Skip.UNSUPPORTED_KEY));
                return null;
            }
            if (button != 0) mouse = button;
            else chord.append(chord.length() == 0 ? "" : "+").append(String.format(Locale.ROOT, "0x%02X", generic(vk)));
        }
        if (mouse != 0) return new TouchLayout.Button(id, name, new int[0], mouse, sticky, x, y, w, h);
        try {
            return new TouchLayout.Button(id, name, KeyboardShortcuts.parseChord(chord.toString()), 0, sticky, x, y, w, h);
        } catch (IllegalArgumentException e) {
            skipped.add(new Skipped(name, Skip.UNSUPPORTED_KEY));
            return null;
        }
    }

    private static TouchLayout placeCustomKeys(JSONObject file, Map<String, JSONObject> keys,
                                               int surfaceWidth, int surfaceHeight, List<Skipped> skipped) {
        // The Custom Keys file has no positions. Lay the keys out as Artemis does: square keys in rows.
        float h = ARTEMIS_KEY, w = Math.min(.8f, ARTEMIS_KEY * surfaceHeight / surfaceWidth);
        int perRow = Math.max(1, (int) ((1 - .02f) / w));
        List<TouchLayout.Button> buttons = new ArrayList<>();
        Set<String> ids = new HashSet<>();
        JSONArray data = file.optJSONArray("data");
        for (int i = 0; data != null && i < data.length(); i++) {
            JSONObject key = data.optJSONObject(i);
            if (key == null) continue;
            String artemisId = key.optString("id", "");
            if (artemisId.isEmpty()) artemisId = Integer.toString(i);
            String name = label(key.optString("name", ""), artemisId);
            int index = buttons.size();
            float x = .01f + (index % perRow) * w, y = Math.min(1 - h, ARTEMIS_KEY + (index / perRow) * h);
            if (index == TouchLayout.MAX_BUTTONS) {
                skipped.add(new Skipped(name, Skip.TOO_MANY));
                continue;
            }
            TouchLayout.Button button = customKey(key, uniqueId("custom_" + artemisId, ids), name, skipped, x, y, w, h);
            if (button != null) buttons.add(button);
        }
        return new TouchLayout(buttons);
    }

    private static String label(String name, String fallback) {
        StringBuilder clean = new StringBuilder();
        for (int i = 0; i < name.length() && clean.length() < 80; i++) {
            char c = name.charAt(i);
            if (!Character.isISOControl(c)) clean.append(c);
        }
        String result = clean.toString().trim();
        if (result.isEmpty()) result = fallback.length() > 80 ? fallback.substring(0, 80) : fallback;
        return result.isEmpty() ? "?" : result;
    }

    private static String uniqueId(String source, Set<String> ids) {
        String base = source.replaceAll("[^A-Za-z0-9_-]", "_");
        if (base.isEmpty()) base = "artemis";
        if (base.length() > 56) base = base.substring(0, 56);
        String id = base;
        for (int n = 2; !ids.add(id); n++) id = base + "-" + n;
        return id;
    }

    private static int mouseButton(int vk) {
        switch (vk) {
            case 0x01: return 1;
            case 0x04: return 2;
            case 0x02: return 3;
            case 0x05: return 4;
            case 0x06: return 5;
            default: return 0;
        }
    }

    // The protocol sends left and right modifiers; Artemis also accepts the generic ones.
    private static int generic(int vk) {
        switch (vk) {
            case 0x10: return 0xA0;
            case 0x11: return 0xA2;
            case 0x12: return 0xA4;
            default: return vk;
        }
    }

    /** A Windows virtual-key code from "0x.." or a "VK_*" name, as Artemis reads them; -1 if unknown. */
    static int virtualKey(String code) {
        try {
            if (code.startsWith("0x") || code.startsWith("0X")) {
                int value = Integer.parseInt(code.substring(2), 16);
                return value > 0 && value <= 0xFF ? value : -1;
            }
        } catch (NumberFormatException e) {
            return -1;
        }
        if (!code.startsWith("VK_")) return -1;
        Integer value = virtualKeys().get(code.substring(3));
        return value == null ? -1 : value;
    }

    private static Map<String, Integer> virtualKeyNames;

    // The VK_* names Artemis resolves (its utils/KeyMapper.java), with the standard Windows values.
    private static synchronized Map<String, Integer> virtualKeys() {
        if (virtualKeyNames != null) return virtualKeyNames;
        String table = "LBUTTON=01 RBUTTON=02 CANCEL=03 MBUTTON=04 XBUTTON1=05 XBUTTON2=06 BACK=08 TAB=09 CLEAR=0C " +
                "RETURN=0D SHIFT=10 CONTROL=11 MENU=12 PAUSE=13 CAPITAL=14 KANA=15 HANGUL=15 IME_ON=16 JUNJA=17 " +
                "FINAL=18 HANJA=19 KANJI=19 IME_OFF=1A ESCAPE=1B CONVERT=1C NONCONVERT=1D ACCEPT=1E MODECHANGE=1F " +
                "SPACE=20 PRIOR=21 NEXT=22 END=23 HOME=24 LEFT=25 UP=26 RIGHT=27 DOWN=28 SELECT=29 PRINT=2A " +
                "EXECUTE=2B SNAPSHOT=2C INSERT=2D DELETE=2E HELP=2F LWIN=5B RWIN=5C APPS=5D SLEEP=5F MULTIPLY=6A " +
                "ADD=6B SEPARATOR=6C SUBTRACT=6D DECIMAL=6E DIVIDE=6F NUMLOCK=90 SCROLL=91 LSHIFT=A0 RSHIFT=A1 " +
                "LCONTROL=A2 RCONTROL=A3 LMENU=A4 RMENU=A5 BROWSER_BACK=A6 BROWSER_FORWARD=A7 BROWSER_REFRESH=A8 " +
                "BROWSER_STOP=A9 BROWSER_SEARCH=AA BROWSER_FAVORITES=AB BROWSER_HOME=AC VOLUME_MUTE=AD " +
                "VOLUME_DOWN=AE VOLUME_UP=AF MEDIA_NEXT_TRACK=B0 MEDIA_PREV_TRACK=B1 MEDIA_STOP=B2 " +
                "MEDIA_PLAY_PAUSE=B3 LAUNCH_MAIL=B4 LAUNCH_MEDIA_SELECT=B5 LAUNCH_APP1=B6 LAUNCH_APP2=B7 OEM_1=BA " +
                "OEM_PLUS=BB OEM_COMMA=BC OEM_MINUS=BD OEM_PERIOD=BE OEM_2=BF OEM_3=C0 OEM_4=DB OEM_5=DC OEM_6=DD " +
                "OEM_7=DE OEM_8=DF OEM_102=E2 PROCESSKEY=E5 PACKET=E7 ATTN=F6 CRSEL=F7 EXSEL=F8 EREOF=F9 PLAY=FA " +
                "ZOOM=FB NONAME=FC PA1=FD OEM_CLEAR=FE";
        Map<String, Integer> names = new HashMap<>();
        for (String entry : table.split(" ")) {
            int split = entry.indexOf('=');
            names.put(entry.substring(0, split), Integer.parseInt(entry.substring(split + 1), 16));
        }
        for (char c = '0'; c <= '9'; c++) names.put(String.valueOf(c), (int) c);
        for (char c = 'A'; c <= 'Z'; c++) names.put(String.valueOf(c), (int) c);
        for (int i = 0; i <= 9; i++) names.put("NUMPAD" + i, 0x60 + i);
        for (int i = 1; i <= 24; i++) names.put("F" + i, 0x6F + i);
        virtualKeyNames = names;
        return names;
    }
}

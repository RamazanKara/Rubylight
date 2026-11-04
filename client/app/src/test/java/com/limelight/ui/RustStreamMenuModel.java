package com.limelight.ui;

import com.limelight.binding.input.KeyboardShortcuts;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Set;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/**
 * The stream menu's model for JVM tests, which cannot load libnightjar_core: the tiles, their places and
 * buttons, the built-ins and the picker are read from nightjar-policy's sources (protocol/policy/src), so a
 * change there reaches these tests, and the layout follows {@code stream_menu::layout}. The Rust tests cover
 * the model itself.
 */
public final class RustStreamMenuModel implements StreamMenuModel.Calls {
    /** A tile id, its place (grid, more, footer) and its button or null, in the model's order. */
    static final class Tile {
        final String id, place, hotkey;
        /** The label's key and English text, or null for a More row. */
        final String labelKey, labelEnglish;

        Tile(String id, String place, String hotkey, String labelKey, String labelEnglish) {
            this.id = id;
            this.place = place;
            this.hotkey = hotkey;
            this.labelKey = labelKey;
            this.labelEnglish = labelEnglish;
        }
    }

    private static final Set<String> MODIFIERS = new HashSet<>(Arrays.asList("CTRL", "SHIFT", "ALT", "WIN",
            "CTRL_RIGHT", "SHIFT_RIGHT", "ALT_RIGHT", "META_RIGHT"));

    final List<Tile> tiles = new ArrayList<>();
    final Map<String, String[]> builtIns = new LinkedHashMap<>();
    final List<String> picker = new ArrayList<>();
    /** Android's stream hotkeys in the table's order: {id}, default keys, old keys or null. */
    final List<String[][]> hotkeys = new ArrayList<>();
    /** Every request, for tests that check what the menu asked. */
    final List<JSONObject> requests = new ArrayList<>();

    static File policySources() {
        for (String path : new String[] {"../protocol/policy/src", "protocol/policy/src"}) {
            File directory = new File(path);
            if (directory.isDirectory()) return directory;
        }
        throw new AssertionError("protocol/policy/src not found from " + new File(".").getAbsolutePath());
    }

    static String read(String name) {
        try {
            return new String(Files.readAllBytes(new File(policySources(), name).toPath()), StandardCharsets.UTF_8);
        } catch (IOException e) {
            throw new AssertionError(e);
        }
    }

    public RustStreamMenuModel() {
        String menu = read("stream_menu.rs");
        String table = menu.substring(menu.indexOf("pub const TILES"), menu.indexOf("];", menu.indexOf("pub const TILES")));
        // grid("bitrate", None, l("quality", "Quality")), more("help"), footer("disconnect", Some(l("disconnect", "Disconnect")))
        Matcher tile = Pattern.compile("(grid|more|footer)\\(\"(\\w+)\"(?:, (?:Some\\(Hotkey::(\\w+)\\)|None))?"
                + "(?:, (?:Some\\()?l\\(\"(\\w+)\", \"([^\"]+)\"\\)\\)?|, None)?\\)").matcher(table);
        while (tile.find()) {
            tiles.add(new Tile(tile.group(2), tile.group(1), tile.group(3) == null ? null : tile.group(3).toLowerCase(Locale.ROOT),
                    tile.group(4), tile.group(5)));
        }
        // MORE => Some(l("more", "More")) in stream_menu::label: the labels of More, Return to stream and Add.
        Matcher extra = Pattern.compile("(MORE|CLOSE|ADD) => Some\\(l\\(\"(\\w+)\", \"([^\"]+)\"\\)\\)").matcher(menu);
        while (extra.find()) tiles.add(new Tile(extra.group(1).toLowerCase(Locale.ROOT), "label", null, extra.group(2), extra.group(3)));
        String shortcuts = read("shortcuts.rs");
        Matcher builtIn = Pattern.compile("BuiltIn \\{ id: \"(\\w+)\", keys: &\\[([^\\]]*)\\] \\}").matcher(shortcuts);
        while (builtIn.find()) builtIns.put(builtIn.group(1), names(builtIn.group(2)));
        int start = shortcuts.indexOf("pub const PICKER");
        picker.addAll(Arrays.asList(names(shortcuts.substring(shortcuts.indexOf("&[", start) + 2, shortcuts.indexOf("];", start)))));
        // hotkey("quit", On::Both, &["CTRL", "ALT", "SHIFT", "Q"]), with old: &[(Client::Android, &[…])] before it
        Matcher hotkey = Pattern.compile("(?:Hotkey \\{ old: &\\[\\(Client::(\\w+), &\\[([^\\]]*)\\]\\)\\], \\.\\.)?"
                + "hotkey\\(\"(\\w+)\", On::(?:Both|Only\\(Client::(\\w+)\\)), &\\[([^\\]]*)\\]\\)").matcher(shortcuts);
        while (hotkey.find()) {
            if (hotkey.group(4) != null && !hotkey.group(4).equals("Android")) continue;
            String[] old = hotkey.group(1) != null && hotkey.group(1).equals("Android") ? names(hotkey.group(2)) : null;
            hotkeys.add(new String[][] {{hotkey.group(3)}, names(hotkey.group(5)), old});
        }
        if (tiles.isEmpty() || builtIns.isEmpty() || picker.isEmpty() || labelled("grid") == 0 || labelled("label") != 3
                || hotkeys.isEmpty()) {
            throw new AssertionError("the policy sources changed shape");
        }
    }

    private int labelled(String place) {
        int count = 0;
        for (Tile tile : tiles) if (tile.place.equals(place) && tile.labelKey != null) count++;
        return count;
    }

    /** Every label key the model gives and its English text, as {@code stream_menu::labels}. */
    Map<String, String> labelTexts() {
        Map<String, String> texts = new LinkedHashMap<>();
        for (Tile tile : tiles) if (tile.labelKey != null) texts.put(tile.labelKey, tile.labelEnglish);
        return texts;
    }

    private static String[] names(String list) {
        List<String> names = new ArrayList<>();
        Matcher name = Pattern.compile("\"([A-Z0-9_]+)\"").matcher(list);
        while (name.find()) names.add(name.group(1));
        return names.toArray(new String[0]);
    }

    static String label(String key) {
        switch (key) {
            case "CTRL": return "Ctrl";
            case "SHIFT": return "Shift";
            case "ALT": return "Alt";
            case "WIN": return "Win";
            case "ESC": return "Esc";
            case "TAB": return "Tab";
            case "DELETE": return "Del";
            case "ENTER": return "Enter";
            case "CAPS_LOCK": return "Caps Lock";
            default: return key;
        }
    }

    @Override
    public String call(String request) {
        try {
            JSONObject in = new JSONObject(request);
            requests.add(in);
            switch (in.getString("op")) {
                case "layout": return ok(layout(in));
                case "built_ins": return ok(builtIns(in.getJSONArray("saved")));
                case "picker": {
                    JSONArray keys = new JSONArray();
                    for (String key : picker) {
                        keys.put(new JSONObject().put("key", key).put("label", label(key))
                                .put("modifier", key.equals("CTRL") || key.equals("SHIFT") || key.equals("ALT") || key.equals("WIN")));
                    }
                    return ok(keys);
                }
                case "validate": {
                    JSONArray keys = in.getJSONArray("keys");
                    if (keys.length() == 0 || keys.length() > KeyboardShortcuts.MAX_KEYS) return error("chord_length");
                    if (in.getString("name").trim().isEmpty()) return error("bad_name");
                    List<String> modifiers = new ArrayList<>(), rest = new ArrayList<>();
                    Set<String> seen = new HashSet<>();
                    for (int i = 0; i < keys.length(); i++) {
                        String key = keys.getString(i).toUpperCase(Locale.ROOT);
                        if (!seen.add(key)) return error("repeated_key");
                        // shortcuts::key_code knows no Android-only names: those the tests use.
                        if (key.startsWith("ANDROID_KEYCODE_") || key.startsWith("MEDIA_")) {
                            return new JSONObject().put("error", "unsupported_key").put("detail", keys.getString(i)).toString();
                        }
                        (MODIFIERS.contains(key) ? modifiers : rest).add(key);
                    }
                    // shortcuts::normalize_keys: modifiers first, each group in the order given.
                    modifiers.addAll(rest);
                    return ok(new JSONArray(modifiers));
                }
                case "chord_label": {
                    JSONArray keys = in.getJSONArray("keys");
                    List<String> labels = new ArrayList<>();
                    List<String> rest = new ArrayList<>();
                    for (int i = 0; i < keys.length(); i++) {
                        (MODIFIERS.contains(keys.getString(i)) ? labels : rest).add(label(keys.getString(i)));
                    }
                    labels.addAll(rest);
                    return ok(String.join(" + ", labels));
                }
                case "labels": {
                    JSONObject labels = new JSONObject();
                    for (Tile tile : tiles) if (tile.labelKey != null) labels.put(tile.id, tile.labelKey);
                    return ok(labels);
                }
                case "mode_line": {
                    String line = modeLine(in.getInt("width"), in.getInt("height"), in.getInt("fps"), in.getInt("format"),
                            in.getBoolean("hdr"));
                    return ok(line == null ? JSONObject.NULL : line);
                }
                case "hotkeys": return ok(hotkeys(in.getJSONObject("bindings")));
                case "check_hotkey": return checkHotkey(in.getString("id"), in.getJSONArray("keys"), in.getJSONObject("bindings"));
                case "check_reset": {
                    // shortcuts::check_reset: the default goes through check_hotkey like a recorded chord.
                    String id = in.getString("id");
                    for (String[][] hotkey : hotkeys) {
                        if (!hotkey[0][0].equals(id)) continue;
                        JSONObject answer = new JSONObject(checkHotkey(id, new JSONArray(Arrays.asList(hotkey[1])), in.getJSONObject("bindings")));
                        return answer.has("error") ? answer.toString() : ok(JSONObject.NULL);
                    }
                    return error("invalid");
                }
                default: return error("invalid");
            }
        } catch (JSONException e) {
            throw new AssertionError(e);
        }
    }

    /**
     * shortcuts::bound: a saved chord or the default, off as null; a chord an earlier action has leaves the later
     * one without it, naming the earlier; an old chord while unchanged and free.
     */
    private JSONArray hotkeys(JSONObject bindings) throws JSONException {
        List<String[]> bound = new ArrayList<>();
        List<String> takenBy = new ArrayList<>();
        for (String[][] hotkey : hotkeys) {
            String saved = bindings.optString(hotkey[0][0], null);
            String[] keys = saved == null ? hotkey[1] : saved.equals("off") ? null : saved.split("\\+");
            String by = null;
            for (int i = 0; i < bound.size() && keys != null; i++) if (Arrays.equals(bound.get(i), keys)) by = hotkeys.get(i)[0][0];
            bound.add(by == null ? keys : null);
            takenBy.add(by);
        }
        JSONArray answer = new JSONArray();
        for (int i = 0; i < hotkeys.size(); i++) {
            String[][] hotkey = hotkeys.get(i);
            boolean changed = bindings.has(hotkey[0][0]);
            JSONArray old = new JSONArray();
            if (!changed && hotkey[2] != null) {
                boolean taken = false;
                for (String[] keys : bound) taken |= keys != null && Arrays.equals(keys, hotkey[2]);
                if (!taken) old.put(chord(hotkey[2]));
            }
            answer.put(new JSONObject().put("id", hotkey[0][0]).put("chord", bound.get(i) == null ? JSONObject.NULL : chord(bound.get(i)))
                    .put("default", chord(hotkey[1])).put("changed", changed).put("old", old)
                    .put("taken", takenBy.get(i) == null ? JSONObject.NULL : takenBy.get(i)));
        }
        return answer;
    }

    private static final Set<String> EDITING = new HashSet<>(Arrays.asList("HOME", "END", "LEFT", "RIGHT", "UP", "DOWN",
            "PAGE_UP", "PAGE_DOWN", "DELETE", "BACKSPACE", "ENTER", "TAB", "INSERT"));

    /** shortcuts::refused_chord, for the keys the tests press: the shell's chords, then the ones used for typing. */
    private static String refused(List<String> modifiers, String key) {
        boolean ctrl = modifiers.contains("CTRL"), alt = modifiers.contains("ALT"), shift = modifiers.contains("SHIFT");
        boolean printable = key.length() == 1 || key.equals("SPACE");
        if ((ctrl && alt && key.equals("DELETE")) || (alt && !ctrl && (key.equals("TAB") || key.equals("ESC")))
                || (ctrl && alt && !shift && key.equals("TAB")) || (ctrl && !alt && key.equals("ESC"))) {
            return "reserved";
        }
        if (shift && !ctrl && !alt && (printable || EDITING.contains(key))) return "hotkey_typing";
        if (alt && !shift && printable) return "hotkey_typing";
        if (ctrl && !alt && !shift && ("ACVXYZ".contains(key) && key.length() == 1 || (EDITING.contains(key) && !key.equals("TAB")))) {
            return "hotkey_typing";
        }
        return null;
    }

    private String checkHotkey(String id, JSONArray given, JSONObject bindings) throws JSONException {
        JSONArray list = given;
        // shortcuts::hotkey_keys: a key alone gets Ctrl+Alt+Shift.
        if (list.length() == 1 && !MODIFIERS.contains(list.getString(0))) {
            list = new JSONArray(Arrays.asList("CTRL", "ALT", "SHIFT", list.getString(0)));
        }
        if (list.length() < 2 || list.length() > 4) return error("hotkey_length");
        List<String> keys = new ArrayList<>();
        String key = null;
        for (String modifier : new String[] {"CTRL", "ALT", "SHIFT"}) {
            for (int i = 0; i < list.length(); i++) if (list.getString(i).equals(modifier)) keys.add(modifier);
        }
        for (int i = 0; i < list.length(); i++) {
            String name = list.getString(i);
            if (name.equals("WIN")) return error("hotkey_modifier");
            if (!keys.contains(name)) {
                if (key != null) return error("hotkey_one_key");
                key = name;
            }
        }
        if (keys.isEmpty()) return error("hotkey_modifier");
        String refusal = refused(keys, key);
        if (refusal != null) return error(refusal);
        keys.add(key);
        JSONArray bound = hotkeys(bindings);
        for (int i = 0; i < bound.length(); i++) {
            JSONObject other = bound.getJSONObject(i);
            if (!other.getString("id").equals(id) && !other.isNull("chord")
                    && Arrays.asList(strings(other.getJSONObject("chord").getJSONArray("keys"))).equals(keys)) {
                return new JSONObject().put("error", "taken").put("detail", other.getString("id")).toString();
            }
        }
        return ok(new JSONArray(keys));
    }

    private static String[] strings(JSONArray array) throws JSONException {
        String[] values = new String[array.length()];
        for (int i = 0; i < values.length; i++) values[i] = array.getString(i);
        return values;
    }

    /** {keys, label, modifiers, vk} as the PC sees the chord: letters, digits, Tab and F1-F24 are enough for the tests. */
    private static JSONObject chord(String[] keys) throws JSONException {
        int modifiers = 0, vk = 0;
        List<String> labels = new ArrayList<>();
        for (String key : keys) {
            labels.add(label(key));
            switch (key) {
                case "SHIFT": modifiers |= 1; break;
                case "CTRL": modifiers |= 2; break;
                case "ALT": modifiers |= 4; break;
                case "TAB": vk = 0x09; break;
                case "DELETE": vk = 0x2E; break;
                default:
                    if (key.length() == 1) vk = key.charAt(0);
                    else if (key.matches("F\\d+")) vk = 0x6F + Integer.parseInt(key.substring(1));
                    else throw new AssertionError("the fake knows no VK for " + key);
            }
        }
        return new JSONObject().put("keys", new JSONArray(Arrays.asList(keys))).put("label", String.join(" + ", labels))
                .put("caps", new JSONArray(labels)).put("modifiers", modifiers).put("vk", vk);
    }

    private Object layout(JSONObject in) throws JSONException {
        Map<String, JSONObject> offers = new LinkedHashMap<>();
        JSONArray list = in.getJSONArray("offers");
        for (int i = 0; i < list.length(); i++) offers.put(list.getJSONObject(i).getString("id"), list.getJSONObject(i));
        for (String id : offers.keySet()) {
            boolean known = false;
            for (Tile tile : tiles) known |= tile.id.equals(id) && !tile.place.equals("label");
            if (!known) return new Unknown(id);
        }
        JSONArray grid = new JSONArray(), more = new JSONArray(), footer = new JSONArray().put("close");
        for (Tile tile : tiles) {
            JSONObject offer = offers.get(tile.id);
            if (offer == null || tile.place.equals("label")) continue;
            if (tile.place.equals("grid")) {
                JSONObject cell = new JSONObject().put("id", tile.id);
                if (offer.has("on")) cell.put("on", offer.getBoolean("on"));
                if (tile.hotkey != null) cell.put("hotkey", tile.hotkey);
                grid.put(new JSONObject().put("tile", cell));
            } else if (tile.place.equals("more")) {
                more.put(tile.id);
            } else {
                footer.put(tile.id);
            }
        }
        if (more.length() > 0) grid.put(new JSONObject().put("tile", new JSONObject().put("id", "more")));
        if (in.has("shortcuts")) {
            int count = Math.min(in.getInt("shortcuts"), KeyboardShortcuts.MAX_SHORTCUTS);
            for (int i = 0; i < count; i++) grid.put(new JSONObject().put("shortcut", i));
            if (count < KeyboardShortcuts.MAX_SHORTCUTS) grid.put("add");
        }
        return new JSONObject().put("grid", grid).put("more", more).put("footer", footer);
    }

    private JSONArray builtIns(JSONArray saved) throws JSONException {
        List<Set<String>> taken = new ArrayList<>();
        for (int i = 0; i < saved.length(); i++) {
            Set<String> keys = new HashSet<>();
            for (int k = 0; k < saved.getJSONArray(i).length(); k++) keys.add(saved.getJSONArray(i).getString(k));
            taken.add(keys);
        }
        JSONArray answer = new JSONArray();
        for (Map.Entry<String, String[]> builtIn : builtIns.entrySet()) {
            if (taken.contains(new HashSet<>(Arrays.asList(builtIn.getValue())))) continue;
            List<String> labels = new ArrayList<>();
            JSONArray keys = new JSONArray();
            for (String key : builtIn.getValue()) {
                labels.add(label(key));
                keys.put(key);
            }
            answer.put(new JSONObject().put("id", builtIn.getKey()).put("label", String.join(" + ", labels)).put("keys", keys));
        }
        return answer;
    }

    // stream_menu::mode_line, for tests: "1440p · 144 FPS · HEVC HDR", the codec dropped first when it runs long.
    static String modeLine(int width, int height, int fps, int format, boolean hdr) {
        if (width == 0 || height == 0 || fps == 0) return null;
        int longSide = Math.max(width, height), shortSide = Math.min(width, height);
        String size = (long) longSide * 900 > (long) shortSide * 1616 ? width + "×" + height
                : shortSide == 2160 ? "4K" : shortSide == 4320 ? "8K" : shortSide + "p";
        String mode = size + " · " + fps + " FPS";
        String codec = (format & 0x0F_0000) != 0 ? "PyroWave" : (format & 0xF000) != 0 ? "AV1"
                : (format & 0x0F00) != 0 ? "HEVC" : (format & 0x000F) != 0 ? "H.264" : null;
        for (String tail : new String[] {codec == null ? null : hdr ? codec + " HDR" : codec, hdr ? "HDR" : null}) {
            if (tail == null) continue;
            String line = mode + " · " + tail;
            if (line.codePointCount(0, line.length()) <= 28) return line;
        }
        return mode;
    }

    private static final class Unknown {
        final String id;

        Unknown(String id) {
            this.id = id;
        }
    }

    private static String ok(Object value) throws JSONException {
        if (value instanceof Unknown) return new JSONObject().put("error", "invalid").put("detail", "unknown tile " + ((Unknown) value).id).toString();
        return new JSONObject().put("ok", value).toString();
    }

    private static String error(String code) throws JSONException {
        return new JSONObject().put("error", code).put("detail", "").toString();
    }
}

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

        Tile(String id, String place, String hotkey) {
            this.id = id;
            this.place = place;
            this.hotkey = hotkey;
        }
    }

    final List<Tile> tiles = new ArrayList<>();
    final Map<String, String[]> builtIns = new LinkedHashMap<>();
    final List<String> picker = new ArrayList<>();
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
        Matcher tile = Pattern.compile("(grid|more|footer)\\(\"(\\w+)\"(?:, (?:Some\\(Hotkey::(\\w+)\\)|None))?\\)").matcher(table);
        while (tile.find()) {
            tiles.add(new Tile(tile.group(2), tile.group(1), tile.group(3) == null ? null : tile.group(3).toLowerCase(Locale.ROOT)));
        }
        String shortcuts = read("shortcuts.rs");
        Matcher builtIn = Pattern.compile("BuiltIn \\{ id: \"(\\w+)\", keys: &\\[([^\\]]*)\\] \\}").matcher(shortcuts);
        while (builtIn.find()) builtIns.put(builtIn.group(1), names(builtIn.group(2)));
        int start = shortcuts.indexOf("pub const PICKER");
        picker.addAll(Arrays.asList(names(shortcuts.substring(shortcuts.indexOf("&[", start) + 2, shortcuts.indexOf("];", start)))));
        if (tiles.isEmpty() || builtIns.isEmpty() || picker.isEmpty()) throw new AssertionError("the policy sources changed shape");
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
                    JSONArray normal = new JSONArray();
                    Set<String> seen = new HashSet<>();
                    for (int i = 0; i < keys.length(); i++) {
                        String key = keys.getString(i).toUpperCase(Locale.ROOT);
                        if (!seen.add(key)) return error("repeated_key");
                        normal.put(key);
                    }
                    return ok(normal);
                }
                case "chord_label": {
                    JSONArray keys = in.getJSONArray("keys");
                    List<String> labels = new ArrayList<>();
                    for (int i = 0; i < keys.length(); i++) labels.add(label(keys.getString(i)));
                    return ok(String.join(" + ", labels));
                }
                default: return error("invalid");
            }
        } catch (JSONException e) {
            throw new AssertionError(e);
        }
    }

    private Object layout(JSONObject in) throws JSONException {
        Map<String, JSONObject> offers = new LinkedHashMap<>();
        JSONArray list = in.getJSONArray("offers");
        for (int i = 0; i < list.length(); i++) offers.put(list.getJSONObject(i).getString("id"), list.getJSONObject(i));
        for (String id : offers.keySet()) {
            boolean known = false;
            for (Tile tile : tiles) known |= tile.id.equals(id);
            if (!known) return new Unknown(id);
        }
        JSONArray grid = new JSONArray(), more = new JSONArray(), footer = new JSONArray().put("close");
        for (Tile tile : tiles) {
            JSONObject offer = offers.get(tile.id);
            if (offer == null) continue;
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

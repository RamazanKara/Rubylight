package com.limelight.ui;

import androidx.annotation.VisibleForTesting;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/**
 * The stream menu's model, the same one desktop Nightjar draws its menu from: nightjar-policy's
 * stream_menu (protocol/policy), reached through libnightjar_core (protocol/android/src/stream_menu.rs).
 * It decides where each offered tile sits, in what order, which controller button runs it, which
 * built-in shortcuts show, the key picker's keys and whether a chord can be saved.
 */
public final class StreamMenuModel {
    private StreamMenuModel() { }

    /** One JSON request in, one JSON answer out: {@code {"ok": …}} or {@code {"error": code, "detail": …}}. */
    public interface Calls {
        String call(String request);
    }

    private static Calls calls;

    private static synchronized Calls calls() {
        if (calls == null) calls = load();
        return calls;
    }

    /** JVM tests cannot load the library; they answer from the Rust sources instead. */
    @VisibleForTesting
    public static synchronized void use(Calls replacement) {
        calls = replacement;
    }

    private static Calls load() {
        try {
            System.loadLibrary("nightjar_core");
            return StreamMenuModel::call;
        } catch (UnsatisfiedLinkError e) {
            String detail = String.valueOf(e.getMessage());
            // Every call then says why, and the menu shows it with the ways out of the stream.
            return request -> {
                try {
                    return new JSONObject().put("error", "unavailable").put("detail", detail).toString();
                } catch (JSONException impossible) {
                    throw new IllegalStateException(impossible);
                }
            };
        }
    }

    private static native String call(String request);

    /** The model refused a request or could not answer it; {@code code} names why. */
    public static final class Refused extends Exception {
        public final String code;
        public final String detail;

        Refused(String code, String detail) {
            super(code + (detail.isEmpty() ? "" : ": " + detail));
            this.code = code;
            this.detail = detail;
        }
    }

    private static Object ask(JSONObject request) throws Refused {
        String answer = calls().call(request.toString());
        try {
            JSONObject reply = new JSONObject(answer == null ? "" : answer);
            if (reply.has("ok")) return reply.get("ok");
            throw new Refused(reply.optString("error", "invalid"), reply.optString("detail", ""));
        } catch (JSONException e) {
            throw new Refused("invalid", "unreadable answer: " + e.getMessage());
        }
    }

    private static JSONObject op(String name) {
        try {
            return new JSONObject().put("op", name);
        } catch (JSONException impossible) {
            throw new IllegalStateException(impossible);
        }
    }

    private static JSONArray array(String[] values) {
        JSONArray array = new JSONArray();
        for (String value : values) array.put(value);
        return array;
    }

    private static String[] strings(JSONArray array) throws JSONException {
        String[] values = new String[array.length()];
        for (int i = 0; i < values.length; i++) values[i] = array.getString(i);
        return values;
    }

    /** A tile the stream can show now, and its state when it is a switch (null for an action). */
    public static final class Offer {
        public final String id;
        public final Boolean on;

        public Offer(String id, Boolean on) {
            this.id = id;
            this.on = on;
        }
    }

    /** One place in the grid: a tile, the user's saved shortcut at {@code shortcut}, or Add. */
    public static final class Cell {
        public enum Kind { TILE, SHORTCUT, ADD }

        public final Kind kind;
        /** For a tile: its id, its state when it is a switch, and the controller button that runs it. */
        public final String id;
        public final Boolean on;
        public final String hotkey;
        public final int shortcut;

        Cell(Kind kind, String id, Boolean on, String hotkey, int shortcut) {
            this.kind = kind;
            this.id = id;
            this.on = on;
            this.hotkey = hotkey;
            this.shortcut = shortcut;
        }
    }

    public static final class Layout {
        public final List<Cell> grid;
        /** One level down, behind the More tile, in order. */
        public final List<String> more;
        /** Close, then the ways out of the session. */
        public final List<String> footer;

        Layout(List<Cell> grid, List<String> more, List<String> footer) {
            this.grid = Collections.unmodifiableList(grid);
            this.more = Collections.unmodifiableList(more);
            this.footer = Collections.unmodifiableList(footer);
        }

        /** The grid cell a controller button ({@code "y"}, {@code "x"}, {@code "lb"}, {@code "rb"}) runs, or -1. */
        public int hotkey(String key) {
            for (int i = 0; i < grid.size(); i++) {
                if (key.equals(grid.get(i).hotkey)) return i;
            }
            return -1;
        }
    }

    /** The first tile ids for what sits one level down and for Close, as the model names them. */
    public static final String MORE = "more";
    public static final String CLOSE = "close";

    /**
     * The menu for these offers. {@code shortcuts} is how many the user saved, or null where they
     * cannot be sent; then neither they nor Add show.
     */
    public static Layout layout(List<Offer> offers, Integer shortcuts) throws Refused {
        try {
            JSONArray list = new JSONArray();
            for (Offer offer : offers) {
                JSONObject item = new JSONObject().put("id", offer.id);
                if (offer.on != null) item.put("on", offer.on.booleanValue());
                list.put(item);
            }
            JSONObject request = op("layout").put("offers", list);
            if (shortcuts != null) request.put("shortcuts", shortcuts.intValue());
            JSONObject answer = (JSONObject) ask(request);
            List<Cell> grid = new ArrayList<>();
            JSONArray cells = answer.getJSONArray("grid");
            for (int i = 0; i < cells.length(); i++) {
                Object cell = cells.get(i);
                if ("add".equals(cell)) {
                    grid.add(new Cell(Cell.Kind.ADD, null, null, null, -1));
                } else if (((JSONObject) cell).has("shortcut")) {
                    grid.add(new Cell(Cell.Kind.SHORTCUT, null, null, null, ((JSONObject) cell).getInt("shortcut")));
                } else {
                    JSONObject tile = ((JSONObject) cell).getJSONObject("tile");
                    grid.add(new Cell(Cell.Kind.TILE, tile.getString("id"),
                            tile.has("on") ? tile.getBoolean("on") : null, tile.optString("hotkey", null), -1));
                }
            }
            List<String> more = new ArrayList<>();
            Collections.addAll(more, strings(answer.getJSONArray("more")));
            List<String> footer = new ArrayList<>();
            Collections.addAll(footer, strings(answer.getJSONArray("footer")));
            return new Layout(grid, more, footer);
        } catch (JSONException | ClassCastException e) {
            throw new Refused("invalid", "unreadable layout: " + e.getMessage());
        }
    }

    /** A built-in shortcut: its id ({@code task_manager}), the keys' text and the keys in their saved names. */
    public static final class BuiltIn {
        public final String id;
        public final String label;
        public final String[] keys;

        BuiltIn(String id, String label, String[] keys) {
            this.id = id;
            this.label = label;
            this.keys = keys;
        }
    }

    /** The built-ins not already saved under a name of their own, given each saved shortcut's keys. */
    public static List<BuiltIn> builtIns(List<String[]> saved) throws Refused {
        try {
            JSONArray keys = new JSONArray();
            for (String[] chord : saved) keys.put(array(chord));
            JSONArray answer = (JSONArray) ask(op("built_ins").put("saved", keys));
            List<BuiltIn> builtIns = new ArrayList<>();
            for (int i = 0; i < answer.length(); i++) {
                JSONObject item = answer.getJSONObject(i);
                builtIns.add(new BuiltIn(item.getString("id"), item.getString("label"), strings(item.getJSONArray("keys"))));
            }
            return builtIns;
        } catch (JSONException | ClassCastException e) {
            throw new Refused("invalid", "unreadable built-ins: " + e.getMessage());
        }
    }

    /** A key the shortcut editor offers: its saved name, its cap's text, and whether it is a modifier. */
    public static final class Key {
        public final String name;
        public final String label;
        public final boolean modifier;

        Key(String name, String label, boolean modifier) {
            this.name = name;
            this.label = label;
            this.modifier = modifier;
        }
    }

    public static List<Key> picker() throws Refused {
        try {
            JSONArray answer = (JSONArray) ask(op("picker"));
            List<Key> keys = new ArrayList<>();
            for (int i = 0; i < answer.length(); i++) {
                JSONObject item = answer.getJSONObject(i);
                keys.add(new Key(item.getString("key"), item.getString("label"), item.getBoolean("modifier")));
            }
            return keys;
        } catch (JSONException | ClassCastException e) {
            throw new Refused("invalid", "unreadable picker: " + e.getMessage());
        }
    }

    /** The keys of a shortcut about to be saved, in their saved names, or why it cannot be saved. */
    public static String[] validate(String name, String[] keys) throws Refused {
        try {
            return strings((JSONArray) ask(op("validate").put("name", name).put("keys", array(keys))));
        } catch (JSONException | ClassCastException e) {
            throw new Refused("invalid", "unreadable check: " + e.getMessage());
        }
    }

    /** A chord's text: {@code Ctrl + Shift + Esc}. */
    public static String chordLabel(String[] keys) throws Refused {
        try {
            return (String) ask(op("chord_label").put("keys", array(keys)));
        } catch (JSONException | ClassCastException e) {
            throw new Refused("invalid", "unreadable label: " + e.getMessage());
        }
    }

    /**
     * The menu header's line for the stream's mode, as desktop Nightjar shows it: {@code 1440p · 144 FPS · HEVC HDR}.
     * Null while the mode is not known (a size or rate of 0). {@code format} is Moonlight's video format bits.
     */
    public static String modeLine(int width, int height, int fps, int format, boolean hdr) throws Refused {
        try {
            Object line = ask(op("mode_line").put("width", width).put("height", height).put("fps", fps)
                    .put("format", format).put("hdr", hdr));
            return line == JSONObject.NULL ? null : (String) line;
        } catch (JSONException | ClassCastException e) {
            throw new Refused("invalid", "unreadable mode line: " + e.getMessage());
        }
    }
}

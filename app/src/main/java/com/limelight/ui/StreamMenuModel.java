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
        String answer;
        try {
            answer = calls().call(request.toString());
        } catch (UnsatisfiedLinkError e) {
            // A library from another build that lacks the call: the menu says so instead of crashing the stream.
            throw new Refused("unavailable", String.valueOf(e.getMessage()));
        } catch (RuntimeException e) {
            throw new Refused("unavailable", String.valueOf(e));
        }
        // JNI answers null when it could not make the answer's string.
        if (answer == null) throw new Refused("unavailable", "no answer");
        try {
            JSONObject reply = new JSONObject(answer);
            if (reply.has("ok")) return reply.get("ok");
            // The model sends a null detail for errors without one.
            throw new Refused(reply.optString("error", "invalid"), reply.isNull("detail") ? "" : reply.optString("detail", ""));
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
        /** The tile id each controller button runs now, in the grid or under it. */
        public final java.util.Map<String, String> buttons;

        Layout(List<Cell> grid, List<String> more, List<String> footer, java.util.Map<String, String> buttons) {
            this.grid = Collections.unmodifiableList(grid);
            this.more = Collections.unmodifiableList(more);
            this.footer = Collections.unmodifiableList(footer);
            this.buttons = Collections.unmodifiableMap(buttons);
        }

        /** The grid cell a controller button ({@code "y"}, {@code "x"}, {@code "lb"}, {@code "rb"}) runs, or -1. */
        public int hotkey(String key) {
            String id = buttons.get(key);
            for (int i = 0; id != null && i < grid.size(); i++) {
                if (id.equals(grid.get(i).id)) return i;
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
        return layout(offers, shortcuts, Collections.emptyMap());
    }

    /** The same with the user's choices for the controller's buttons, by button and tile id. */
    public static Layout layout(List<Offer> offers, Integer shortcuts, java.util.Map<String, String> buttons) throws Refused {
        try {
            JSONArray list = new JSONArray();
            for (Offer offer : offers) {
                JSONObject item = new JSONObject().put("id", offer.id);
                if (offer.on != null) item.put("on", offer.on.booleanValue());
                list.put(item);
            }
            JSONObject request = op("layout").put("offers", list);
            if (shortcuts != null) request.put("shortcuts", shortcuts.intValue());
            if (!buttons.isEmpty()) request.put("buttons", new JSONObject(buttons));
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
            java.util.Map<String, String> running = new java.util.HashMap<>();
            JSONObject chosen = answer.getJSONObject("buttons");
            for (java.util.Iterator<String> keys = chosen.keys(); keys.hasNext(); ) {
                String key = keys.next();
                running.put(key, chosen.getString(key));
            }
            return new Layout(grid, more, footer, running);
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

    /** A stream hotkey's chord: its saved key names, its text, and what the PC would see (modifier bits, Windows VK). */
    public static final class Chord {
        public final String[] keys;
        public final String label;
        /** Each key's English name in the label's order, for the app to say in the user's language. */
        public final String[] caps;
        public final int modifiers;
        public final int vk;

        Chord(JSONObject chord) throws JSONException {
            keys = strings(chord.getJSONArray("keys"));
            label = chord.getString("label");
            caps = strings(chord.getJSONArray("caps"));
            modifiers = chord.getInt("modifiers");
            vk = chord.getInt("vk");
        }

        private static Chord of(JSONObject owner, String name) throws JSONException {
            return owner.isNull(name) ? null : new Chord(owner.getJSONObject(name));
        }
    }

    /**
     * One of Android's stream hotkeys from the table both clients share: its chord now (null when off), its
     * default, whether the user changed it, and the chords it answered to before, which still work while it is unchanged.
     * Two actions saved with one chord leave the later without one: {@code taken} names the action that has it.
     */
    public static final class Hotkey {
        public final String id;
        public final Chord chord;
        public final Chord standard;
        public final boolean changed;
        public final List<Chord> old;
        public final String taken;

        Hotkey(JSONObject item) throws JSONException {
            id = item.getString("id");
            chord = Chord.of(item, "chord");
            standard = new Chord(item.getJSONObject("default"));
            changed = item.getBoolean("changed");
            taken = item.isNull("taken") ? null : item.getString("taken");
            List<Chord> before = new ArrayList<>();
            JSONArray list = item.getJSONArray("old");
            for (int i = 0; i < list.length(); i++) before.add(new Chord(list.getJSONObject(i)));
            old = Collections.unmodifiableList(before);
        }
    }

    private static JSONObject bindings(java.util.Map<String, String> bindings) throws JSONException {
        JSONObject object = new JSONObject();
        for (java.util.Map.Entry<String, String> entry : bindings.entrySet()) object.put(entry.getKey(), entry.getValue());
        return object;
    }

    /** Android's stream hotkeys with the user's changes ({@code id → "CTRL+F2"} or {@code "off"}), in the table's order. */
    public static List<Hotkey> hotkeys(java.util.Map<String, String> bindings) throws Refused {
        try {
            JSONArray answer = (JSONArray) ask(op("hotkeys").put("bindings", bindings(bindings)));
            List<Hotkey> hotkeys = new ArrayList<>();
            for (int i = 0; i < answer.length(); i++) hotkeys.add(new Hotkey(answer.getJSONObject(i)));
            return hotkeys;
        } catch (JSONException | ClassCastException e) {
            throw new Refused("invalid", "unreadable hotkeys: " + e.getMessage());
        }
    }

    /** A hotkey's new keys in their saved order, or why they are refused ({@code taken} names the action that has them). */
    public static String[] checkHotkey(String id, String[] keys, java.util.Map<String, String> bindings) throws Refused {
        try {
            return strings((JSONArray) ask(op("check_hotkey").put("id", id).put("keys", array(keys)).put("bindings", bindings(bindings))));
        } catch (JSONException | ClassCastException e) {
            throw new Refused("invalid", "unreadable hotkey check: " + e.getMessage());
        }
    }

    /** Whether a hotkey may go back to its default; refused ({@code taken}) while another action has that chord now. */
    public static void checkReset(String id, java.util.Map<String, String> bindings) throws Refused {
        try {
            ask(op("check_reset").put("id", id).put("bindings", bindings(bindings)));
        } catch (JSONException e) {
            throw new Refused("invalid", "unreadable reset check: " + e.getMessage());
        }
    }

    /** The model's id for the grid's Add tile, as {@link #labels} names it. */
    public static final String ADD = "add";

    /**
     * Each tile id's label key ({@code bitrate} → {@code quality}), for the grid, More, Add, Return to stream
     * ({@link #CLOSE}) and the ways out; the label is the {@code stream_tile_<key>} string, as desktop Nightjar's.
     */
    public static java.util.Map<String, String> labels() throws Refused {
        try {
            JSONObject answer = (JSONObject) ask(op("labels"));
            java.util.Map<String, String> labels = new java.util.HashMap<>();
            for (java.util.Iterator<String> ids = answer.keys(); ids.hasNext(); ) {
                String id = ids.next();
                labels.put(id, answer.getString(id));
            }
            return labels;
        } catch (JSONException | ClassCastException e) {
            throw new Refused("invalid", "unreadable labels: " + e.getMessage());
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

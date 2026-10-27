package com.limelight.binding.input;

import android.content.Context;
import com.limelight.nvstream.input.ControllerPacket;
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
import java.util.Set;

/** Portable game controls. Positions and sizes are fractions of the usable control surface. */
public final class TouchLayout {
    public static final int MAX_BUTTONS = 32, MAX_BYTES = 64 * 1024;
    /** Version 2 adds gamepad, trigger, stick, D-pad and WASD controls. Layouts without them still save as 1. */
    public static final int VERSION = 2;
    public static final int LEFT = 0, RIGHT = 1;
    public static final float DEFAULT_DEADZONE = .1f, DEFAULT_DIAGONAL = 45, MAX_DEADZONE = .5f, MAX_DIAGONAL = 90;
    public static final String[] GAMEPAD_BUTTONS = {"A", "B", "X", "Y", "LB", "RB", "LS", "RS", "BACK", "START", "GUIDE"};
    private static final int[] GAMEPAD_FLAGS = {ControllerPacket.A_FLAG, ControllerPacket.B_FLAG,
            ControllerPacket.X_FLAG, ControllerPacket.Y_FLAG, ControllerPacket.LB_FLAG, ControllerPacket.RB_FLAG,
            ControllerPacket.LS_CLK_FLAG, ControllerPacket.RS_CLK_FLAG, ControllerPacket.BACK_FLAG,
            ControllerPacket.PLAY_FLAG, ControllerPacket.SPECIAL_BUTTON_FLAG};
    private static final String PREFS = "GameTouchLayouts", SNAP_GRID = "snap_grid";
    /** Grid steps in tenths of a percent of the shorter edge; 0 is off. */
    public static final int[] SNAP_GRIDS = {0, 25, 50, 100};
    public final List<Button> buttons;

    public TouchLayout(List<Button> buttons) {
        if (buttons == null || buttons.size() > MAX_BUTTONS) throw new InvalidLayoutException(InvalidLayoutException.Reason.TOO_LARGE, 0, null);
        Set<String> ids = new HashSet<>();
        for (Button button : buttons) {
            if (!ids.add(button.id)) throw new InvalidLayoutException(InvalidLayoutException.Reason.DUPLICATE, 0, button.id);
        }
        this.buttons = java.util.Collections.unmodifiableList(new ArrayList<>(buttons));
    }

    /** Why a layout was rejected; {@code control} counts from 1 and is 0 for the whole document. */
    public static final class InvalidLayoutException extends IllegalArgumentException {
        public enum Reason { NOT_JSON, VERSION, TOO_LARGE, DUPLICATE, CONTROL }
        public final Reason reason;
        public final int control;
        public final String detail;

        public InvalidLayoutException(Reason reason, int control, String detail) {
            super(reason + (control > 0 ? " at control " + control : "") + (detail == null ? "" : ": " + detail));
            this.reason = reason; this.control = control; this.detail = detail;
        }
    }

    public enum Kind {
        KEYBOARD("keyboard"), MOUSE("mouse"), GAMEPAD("gamepad"), TRIGGER("trigger"), STICK("stick"), DPAD("dpad"), WASD("wasd");
        public final String json;
        Kind(String json) { this.json = json; }
        /** Follows a finger around a center instead of acting as a button. */
        public boolean directional() { return this == STICK || this == DPAD || this == WASD; }
        public boolean gamepad() { return this == GAMEPAD || this == TRIGGER || this == STICK || this == DPAD; }
        static Kind of(String json) {
            for (Kind kind : values()) if (kind.json.equals(json)) return kind;
            return null;
        }
    }

    public static final class Button {
        public final String id, name;
        public final Kind kind;
        public final float x, y, width, height;
        public final boolean toggle;
        public final int mouseButton;
        /** {@link ControllerPacket} flag of a gamepad button. */
        public final int gamepadButton;
        /** {@link #LEFT} or {@link #RIGHT} for sticks and triggers. */
        public final int side;
        /** Sticks: the center is wherever the thumb lands inside the control. */
        public final boolean floating;
        /** Fraction of the stick radius that sends nothing. */
        public final float deadzone;
        /** WASD sticks: degrees of each diagonal in which both neighbouring keys are held. */
        public final float diagonal;
        // Keyboard chord, or the up, left, down and right keys of a WASD stick.
        private final int[] keys;

        public Button(String id, String name, int[] keys, int mouseButton, boolean toggle,
                      float x, float y, float width, float height) {
            this(mouseButton == 0 ? Kind.KEYBOARD : Kind.MOUSE, id, name, keys, mouseButton, 0, LEFT, toggle,
                    false, 0, 0, x, y, width, height);
        }

        private Button(Kind kind, String id, String name, int[] keys, int mouseButton, int gamepadButton, int side,
                       boolean toggle, boolean floating, float deadzone, float diagonal,
                       float x, float y, float width, float height) {
            if (keys == null || kind == null) throw new IllegalArgumentException("Missing binding");
            // Reuse the shortcut validator for IDs, labels and supported key codes.
            new KeyboardShortcuts.Shortcut(id, name, kind == Kind.KEYBOARD || kind == Kind.WASD ? keys : new int[] {29});
            boolean binding;
            switch (kind) {
                case KEYBOARD: binding = mouseButton == 0; break;
                case MOUSE: binding = mouseButton >= 1 && mouseButton <= 5 && keys.length == 0; break;
                case GAMEPAD: binding = gamepadName(gamepadButton) != null && keys.length == 0; break;
                case WASD: binding = keys.length == 4 && !toggle; break;
                case TRIGGER: binding = keys.length == 0; break;
                default: binding = keys.length == 0 && !toggle; break;
            }
            if (!binding || (kind != Kind.MOUSE && mouseButton != 0) || (kind != Kind.GAMEPAD && gamepadButton != 0) ||
                    (side != LEFT && side != RIGHT) || !finite(x, y, width, height, deadzone, diagonal) ||
                    deadzone < 0 || deadzone > MAX_DEADZONE || diagonal < 0 || diagonal > MAX_DIAGONAL ||
                    x < 0 || y < 0 || width < .04f || height < .04f ||
                    width > .8f || height > .8f || x + width > 1.00001f || y + height > 1.00001f) {
                throw new IllegalArgumentException("Invalid control geometry or binding");
            }
            this.kind = kind; this.id = id; this.name = name.trim(); this.keys = keys.clone();
            this.mouseButton = mouseButton; this.gamepadButton = gamepadButton; this.side = side;
            this.toggle = toggle; this.floating = floating; this.deadzone = deadzone; this.diagonal = diagonal;
            this.x = x; this.y = y; this.width = width; this.height = height;
        }

        // One direction of a WASD stick, held through the same reference counts as keyboard buttons.
        private Button(Button stick, int direction) {
            kind = Kind.KEYBOARD; id = stick.id + '\0' + direction; name = stick.name; keys = new int[] {stick.keys[direction]};
            mouseButton = 0; gamepadButton = 0; side = LEFT; toggle = false; floating = false; deadzone = 0; diagonal = 0;
            x = stick.x; y = stick.y; width = stick.width; height = stick.height;
        }

        public static Button gamepad(String id, String name, int flag, boolean toggle, float x, float y, float width, float height) {
            return new Button(Kind.GAMEPAD, id, name, new int[0], 0, flag, LEFT, toggle, false, 0, 0, x, y, width, height);
        }
        public static Button trigger(String id, String name, int side, boolean toggle, float x, float y, float width, float height) {
            return new Button(Kind.TRIGGER, id, name, new int[0], 0, 0, side, toggle, false, 0, 0, x, y, width, height);
        }
        public static Button stick(String id, String name, int side, boolean floating, float deadzone,
                                   float x, float y, float width, float height) {
            return new Button(Kind.STICK, id, name, new int[0], 0, 0, side, false, floating, deadzone, 0, x, y, width, height);
        }
        public static Button dpad(String id, String name, float x, float y, float width, float height) {
            return new Button(Kind.DPAD, id, name, new int[0], 0, 0, LEFT, false, false, 0, 0, x, y, width, height);
        }
        /** Keys in the order up, left, down, right. */
        public static Button wasd(String id, String name, int[] keys, boolean floating, float deadzone, float diagonal,
                                  float x, float y, float width, float height) {
            return new Button(Kind.WASD, id, name, keys, 0, 0, LEFT, false, floating, deadzone, diagonal, x, y, width, height);
        }

        public int[] keys() { return keys.clone(); }
        /** The keyboard button for one WASD direction (0 up, 1 left, 2 down, 3 right). */
        public Button directionKey(int direction) {
            if (kind != Kind.WASD) throw new IllegalStateException("Not a WASD stick");
            return new Button(this, direction);
        }
        public Button move(float x, float y) {
            return new Button(kind, id, name, keys, mouseButton, gamepadButton, side, toggle, floating, deadzone, diagonal,
                    Math.max(0, Math.min(1 - width, x)), Math.max(0, Math.min(1 - height, y)), width, height);
        }
    }

    public static String gamepadName(int flag) {
        for (int i = 0; i < GAMEPAD_FLAGS.length; i++) if (GAMEPAD_FLAGS[i] == flag) return GAMEPAD_BUTTONS[i];
        return null;
    }
    public static int gamepadFlag(String name) {
        for (int i = 0; i < GAMEPAD_BUTTONS.length; i++) if (GAMEPAD_BUTTONS[i].equals(name)) return GAMEPAD_FLAGS[i];
        return 0;
    }

    private static boolean finite(float... values) {
        for (float value : values) if (Float.isNaN(value) || Float.isInfinite(value)) return false;
        return true;
    }

    public static String profileKey(String hostUuid, String appUuid, int appId) {
        String host = hostUuid == null ? "" : hostUuid;
        String app = appUuid == null || appUuid.isEmpty() ? "id:" + appId : "uuid:" + appUuid;
        // Length-prefixing prevents collisions even if either ID contains a delimiter.
        return host.length() + ":" + host + app;
    }

    public static TouchLayout empty() { return new TouchLayout(new ArrayList<>()); }
    public static TouchLayout load(Context context, String profile) {
        try { return parse(context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(profile, "{\"version\":1,\"buttons\":[]}")); }
        catch (IllegalArgumentException e) { return empty(); }
    }
    public void save(Context context, String profile) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putString(profile, encode()).apply();
    }
    public static boolean enabled(Context context, String profile) {
        return context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean("enabled:" + profile, false);
    }
    public static void setEnabled(Context context, String profile, boolean enabled) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putBoolean("enabled:" + profile, enabled).apply();
    }
    /** The editor's grid step for every game on this device, from {@link #SNAP_GRIDS}. */
    public static int snapGrid(Context context) {
        int grid = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getInt(SNAP_GRID, 0);
        for (int allowed : SNAP_GRIDS) if (allowed == grid) return grid;
        return 0;
    }
    public static void setSnapGrid(Context context, int grid) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putInt(SNAP_GRID, grid).apply();
    }
    public static String readText(InputStream input) throws IOException {
        if (input == null) throw new IOException("Cannot open layout");
        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        byte[] buffer = new byte[4096];
        int count;
        while ((count = input.read(buffer, 0, Math.min(buffer.length, MAX_BYTES + 1 - bytes.size()))) != -1) {
            bytes.write(buffer, 0, count);
            if (bytes.size() > MAX_BYTES) throw new IOException("Layout exceeds 64 KiB");
        }
        return new String(bytes.toByteArray(), StandardCharsets.UTF_8);
    }
    public static TouchLayout read(InputStream input) throws IOException {
        return parse(readText(input));
    }
    public static TouchLayout parse(String json) {
        if (json == null || json.getBytes(StandardCharsets.UTF_8).length > MAX_BYTES) {
            throw new InvalidLayoutException(InvalidLayoutException.Reason.TOO_LARGE, 0, null);
        }
        JSONObject document;
        JSONArray buttons;
        int version;
        try {
            document = new JSONObject(json);
            Object value = document.get("version");
            double number = value instanceof Number ? ((Number) value).doubleValue() : Double.NaN;
            if (number != 1 && number != VERSION) {
                throw new InvalidLayoutException(InvalidLayoutException.Reason.VERSION, 0, String.valueOf(value));
            }
            version = (int) number;
            buttons = document.getJSONArray("buttons");
        } catch (JSONException e) {
            throw new InvalidLayoutException(InvalidLayoutException.Reason.NOT_JSON, 0, e.getMessage());
        }
        if (buttons.length() > MAX_BUTTONS) throw new InvalidLayoutException(InvalidLayoutException.Reason.TOO_LARGE, 0, null);
        List<Button> result = new ArrayList<>();
        for (int i = 0; i < buttons.length(); i++) {
            JSONObject b = buttons.optJSONObject(i);
            String label = b == null ? null : b.optString("name", b.optString("id", null));
            try {
                if (b == null) throw new IllegalArgumentException("Not an object");
                result.add(parseButton(b, version));
            } catch (JSONException | IllegalArgumentException e) {
                throw new InvalidLayoutException(InvalidLayoutException.Reason.CONTROL, i + 1, label);
            }
        }
        return new TouchLayout(result);
    }

    private static Button parseButton(JSONObject b, int version) throws JSONException {
        Kind kind = Kind.of(b.getString("type"));
        if (kind == null || (version == 1 && kind != Kind.KEYBOARD && kind != Kind.MOUSE)) {
            throw new IllegalArgumentException("Unsupported binding");
        }
        String id = b.getString("id"), name = b.getString("name");
        float x = (float) b.getDouble("x"), y = (float) b.getDouble("y");
        float width = (float) b.getDouble("width"), height = (float) b.getDouble("height");
        switch (kind) {
            case MOUSE:
                int mouse = b.getInt("button");
                if (mouse == 0) throw new IllegalArgumentException("Invalid mouse button");
                return new Button(id, name, new int[0], mouse, b.getBoolean("toggle"), x, y, width, height);
            case GAMEPAD:
                int flag = gamepadFlag(b.getString("button"));
                if (flag == 0) throw new IllegalArgumentException("Unknown gamepad button");
                return Button.gamepad(id, name, flag, b.getBoolean("toggle"), x, y, width, height);
            case TRIGGER:
                return Button.trigger(id, name, side(b), b.getBoolean("toggle"), x, y, width, height);
            case STICK:
                return Button.stick(id, name, side(b), b.getBoolean("floating"), (float) b.getDouble("deadzone"), x, y, width, height);
            case DPAD:
                return Button.dpad(id, name, x, y, width, height);
            case WASD:
                int[] keys = KeyboardShortcuts.parseChord(b.getString("up") + "+" + b.getString("left") + "+" +
                        b.getString("down") + "+" + b.getString("right"));
                return Button.wasd(id, name, keys, b.getBoolean("floating"), (float) b.getDouble("deadzone"),
                        (float) b.getDouble("diagonal"), x, y, width, height);
            default:
                return new Button(id, name, KeyboardShortcuts.parseChord(b.getString("keys")), 0, b.getBoolean("toggle"),
                        x, y, width, height);
        }
    }

    private static int side(JSONObject b) throws JSONException {
        String side = b.getString("side");
        if (side.equals("left")) return LEFT;
        if (side.equals("right")) return RIGHT;
        throw new IllegalArgumentException("Unknown side");
    }

    public String encode() {
        try {
            JSONArray data = new JSONArray();
            int version = 1;
            for (Button b : buttons) {
                JSONObject item = new JSONObject().put("id", b.id).put("name", b.name).put("type", b.kind.json);
                if (!b.kind.directional()) item.put("toggle", b.toggle);
                item.put("x", b.x).put("y", b.y).put("width", b.width).put("height", b.height);
                switch (b.kind) {
                    case KEYBOARD: item.put("keys", KeyboardShortcuts.format(b.keys)); break;
                    case MOUSE: item.put("button", b.mouseButton); break;
                    case GAMEPAD: item.put("button", gamepadName(b.gamepadButton)); break;
                    case TRIGGER: item.put("side", b.side == LEFT ? "left" : "right"); break;
                    case STICK: item.put("side", b.side == LEFT ? "left" : "right").put("floating", b.floating)
                            .put("deadzone", b.deadzone); break;
                    case WASD:
                        String[] names = {"up", "left", "down", "right"};
                        for (int i = 0; i < 4; i++) item.put(names[i], KeyboardShortcuts.format(new int[] {b.keys[i]}));
                        item.put("floating", b.floating).put("deadzone", b.deadzone).put("diagonal", b.diagonal);
                        break;
                    default: break;
                }
                if (b.kind != Kind.KEYBOARD && b.kind != Kind.MOUSE) version = VERSION;
                data.put(item);
            }
            return new JSONObject().put("version", version).put("buttons", data).toString(2);
        } catch (JSONException e) { throw new IllegalArgumentException("Invalid layout", e); }
    }
}

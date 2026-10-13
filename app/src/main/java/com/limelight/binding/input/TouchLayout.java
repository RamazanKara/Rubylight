package com.limelight.binding.input;

import android.content.Context;
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
    private static final String PREFS = "GameTouchLayouts";
    public final List<Button> buttons;

    public TouchLayout(List<Button> buttons) {
        if (buttons == null || buttons.size() > MAX_BUTTONS) throw new IllegalArgumentException("Too many controls");
        Set<String> ids = new HashSet<>();
        for (Button button : buttons) if (!ids.add(button.id)) throw new IllegalArgumentException("Duplicate control");
        this.buttons = java.util.Collections.unmodifiableList(new ArrayList<>(buttons));
    }

    public static final class Button {
        public final String id, name;
        public final float x, y, width, height;
        public final boolean toggle;
        public final int mouseButton;
        private final int[] keys;

        public Button(String id, String name, int[] keys, int mouseButton, boolean toggle,
                      float x, float y, float width, float height) {
            if (keys == null) throw new IllegalArgumentException("Missing keys");
            // Reuse the shortcut validator for IDs, labels and supported key codes.
            new KeyboardShortcuts.Shortcut(id, name, mouseButton == 0 ? keys : new int[] {29});
            if (mouseButton < 0 || mouseButton > 5 || (mouseButton != 0 && keys.length != 0) ||
                    !finite(x, y, width, height) || x < 0 || y < 0 || width < .04f || height < .04f ||
                    width > .8f || height > .8f || x + width > 1.00001f || y + height > 1.00001f) {
                throw new IllegalArgumentException("Invalid control geometry or binding");
            }
            this.id = id; this.name = name.trim(); this.keys = keys.clone(); this.mouseButton = mouseButton;
            this.toggle = toggle; this.x = x; this.y = y; this.width = width; this.height = height;
        }
        public int[] keys() { return keys.clone(); }
        public Button move(float x, float y) {
            return new Button(id, name, keys, mouseButton, toggle, Math.max(0, Math.min(1 - width, x)),
                    Math.max(0, Math.min(1 - height, y)), width, height);
        }
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
    public static TouchLayout read(InputStream input) throws IOException {
        if (input == null) throw new IOException("Cannot open layout");
        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        byte[] buffer = new byte[4096];
        int count;
        while ((count = input.read(buffer, 0, Math.min(buffer.length, MAX_BYTES + 1 - bytes.size()))) != -1) {
            bytes.write(buffer, 0, count);
            if (bytes.size() > MAX_BYTES) throw new IOException("Layout exceeds 64 KiB");
        }
        return parse(new String(bytes.toByteArray(), StandardCharsets.UTF_8));
    }
    public static TouchLayout parse(String json) {
        if (json == null || json.getBytes(StandardCharsets.UTF_8).length > MAX_BYTES) throw new IllegalArgumentException("Layout exceeds 64 KiB");
        try {
            JSONObject document = new JSONObject(json);
            Object version = document.get("version");
            if (!(version instanceof Number) || ((Number) version).doubleValue() != 1) {
                throw new IllegalArgumentException("Unsupported layout version");
            }
            JSONArray buttons = document.getJSONArray("buttons");
            if (buttons.length() > MAX_BUTTONS) throw new IllegalArgumentException("Too many controls");
            List<Button> result = new ArrayList<>();
            for (int i = 0; i < buttons.length(); i++) {
                JSONObject b = buttons.getJSONObject(i);
                String type = b.getString("type");
                if (!type.equals("keyboard") && !type.equals("mouse")) throw new IllegalArgumentException("Unsupported binding");
                int mouse = type.equals("mouse") ? b.getInt("button") : 0;
                if (type.equals("mouse") && mouse == 0) throw new IllegalArgumentException("Invalid mouse button");
                int[] keys = mouse == 0 ? KeyboardShortcuts.parseChord(b.getString("keys")) : new int[0];
                result.add(new Button(b.getString("id"), b.getString("name"), keys, mouse, b.getBoolean("toggle"),
                        (float)b.getDouble("x"), (float)b.getDouble("y"), (float)b.getDouble("width"), (float)b.getDouble("height")));
            }
            return new TouchLayout(result);
        } catch (JSONException e) { throw new IllegalArgumentException("Invalid layout", e); }
    }
    public String encode() {
        try {
            JSONArray data = new JSONArray();
            for (Button b : buttons) {
                JSONObject item = new JSONObject().put("id", b.id).put("name", b.name)
                        .put("type", b.mouseButton == 0 ? "keyboard" : "mouse").put("toggle", b.toggle)
                        .put("x", b.x).put("y", b.y).put("width", b.width).put("height", b.height);
                if (b.mouseButton == 0) item.put("keys", KeyboardShortcuts.format(b.keys));
                else item.put("button", b.mouseButton);
                data.put(item);
            }
            return new JSONObject().put("version", 1).put("buttons", data).toString(2);
        } catch (JSONException e) { throw new IllegalArgumentException("Invalid layout", e); }
    }
}

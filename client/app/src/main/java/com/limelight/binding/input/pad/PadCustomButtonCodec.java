package com.limelight.binding.input.pad;

import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Locale;
import java.util.Set;

/**
 * Pure-JVM backward-compatible codec for {@link PadTouchZone} ({@code zone=...}) and
 * {@link PadCustomButton} ({@code kbm=...}) tokens inside {@link PadLayoutCodec}, plus JSON
 * serialization ({@link #toJson}/{@link #applyJson}) ready for Hub Sync.
 */
public final class PadCustomButtonCodec {
    private static final String ZONE_PREFIX = "zone=";
    private static final String KBM_PREFIX = "kbm=";

    private PadCustomButtonCodec() {
    }

    /** Appends {@code |zone=...} and {@code |kbm=...} tokens; leaves output empty when none exist. */
    static void appendTokens(PadLayout layout, StringBuilder out) {
        List<PadTouchZone> zones = layout.touchZones();
        int zoneLimit = Math.min(PadTouchZone.MAX_ZONES, zones.size());
        for (int i = 0; i < zoneLimit; i++) {
            PadTouchZone z = zones.get(i);
            out.append('|').append(ZONE_PREFIX).append(escape(z.id)).append(',')
                    .append(z.kind.name().toLowerCase(Locale.ROOT)).append(',')
                    .append(String.format(Locale.ROOT, "%.4f", z.left)).append(',')
                    .append(String.format(Locale.ROOT, "%.4f", z.top)).append(',')
                    .append(String.format(Locale.ROOT, "%.4f", z.right)).append(',')
                    .append(String.format(Locale.ROOT, "%.4f", z.bottom)).append(',')
                    .append(z.opacity).append(',')
                    .append(z.autoSprint ? '1' : '0').append(',')
                    .append(String.format(Locale.ROOT, "%.2f", z.sprintThreshold)).append(',')
                    .append(String.format(Locale.ROOT, "%.2f", z.deadzone)).append(',')
                    .append(z.sensitivityPercent).append(',')
                    .append(z.invertY ? '1' : '0').append(',')
                    .append(PadTouchZone.normalizeCurve(z.accelerationCurve)).append(',')
                    .append(z.tapAction.name().toLowerCase(Locale.ROOT));
        }
        List<PadCustomButton> buttons = layout.customButtons();
        int buttonLimit = Math.min(PadCustomButton.MAX_CUSTOM_BUTTONS, buttons.size());
        for (int i = 0; i < buttonLimit; i++) {
            PadCustomButton b = buttons.get(i);
            out.append('|').append(KBM_PREFIX).append(escape(b.id)).append(',')
                    .append(b.shape.name().toLowerCase(Locale.ROOT)).append(',')
                    .append(b.bindingKind.name().toLowerCase(Locale.ROOT)).append(',')
                    .append(formatBindingSpec(b)).append(',')
                    .append(String.format(Locale.ROOT, "%.4f", b.cx)).append(',')
                    .append(String.format(Locale.ROOT, "%.4f", b.cy)).append(',')
                    .append(String.format(Locale.ROOT, "%.3f", b.widthScale)).append(',')
                    .append(String.format(Locale.ROOT, "%.3f", b.heightScale)).append(',')
                    .append(b.opacity).append(',')
                    .append(b.toggle ? '1' : '0').append(',')
                    .append(b.dragAim ? '1' : '0').append(',')
                    .append(escape(b.label));
        }
    }

    /** Parses a single {@code zone=...} or {@code kbm=...} token into {@code layout}; returns true if recognized. */
    static boolean parseToken(String token, PadLayout layout) {
        if (token.startsWith(ZONE_PREFIX)) {
            if (layout.touchZones().size() >= PadTouchZone.MAX_ZONES) return true;
            PadTouchZone z = parseZone(token.substring(ZONE_PREFIX.length()));
            if (z != null) layout.addTouchZone(z);
            return true;
        }
        if (token.startsWith(KBM_PREFIX)) {
            if (layout.customButtons().size() >= PadCustomButton.MAX_CUSTOM_BUTTONS) return true;
            PadCustomButton b = parseButton(token.substring(KBM_PREFIX.length()));
            if (b != null) layout.addCustomButton(b);
            return true;
        }
        return false;
    }

    private static PadTouchZone parseZone(String body) {
        String[] fields = body.split(",", -1);
        if (fields.length < 6) return null;
        try {
            String id = PadTouchZone.sanitizeId(unescape(fields[0]), null);
            if (id == null) return null;
            PadTouchZone.Kind kind = PadTouchZone.Kind.valueOf(fields[1].trim().toUpperCase(Locale.ROOT));
            float left = Float.parseFloat(fields[2]);
            float top = Float.parseFloat(fields[3]);
            float right = Float.parseFloat(fields[4]);
            float bottom = Float.parseFloat(fields[5]);
            PadTouchZone z = new PadTouchZone(id, kind, left, top, right, bottom);
            if (fields.length >= 7 && !fields[6].isEmpty()) z.setOpacity(Integer.parseInt(fields[6]));
            if (fields.length >= 8 && !fields[7].isEmpty()) z.autoSprint = "1".equals(fields[7]);
            if (fields.length >= 9 && !fields[8].isEmpty()) z.setSprintThreshold(Float.parseFloat(fields[8]));
            if (fields.length >= 10 && !fields[9].isEmpty()) z.setDeadzone(Float.parseFloat(fields[9]));
            if (fields.length >= 11 && !fields[10].isEmpty()) z.setSensitivityPercent(Integer.parseInt(fields[10]));
            if (fields.length >= 12 && !fields[11].isEmpty()) z.invertY = "1".equals(fields[11]);
            if (fields.length >= 13 && !fields[12].isEmpty()) z.setAccelerationCurve(fields[12].toLowerCase(Locale.ROOT));
            if (fields.length >= 14 && !fields[13].isEmpty()) {
                z.tapAction = PadTouchZone.TapAction.valueOf(fields[13].trim().toUpperCase(Locale.ROOT));
            }
            return z;
        } catch (IllegalArgumentException e) {
            return null;
        }
    }

    private static PadCustomButton parseButton(String body) {
        String[] fields = body.split(",", -1);
        if (fields.length < 11) return null;
        try {
            String id = PadTouchZone.sanitizeId(unescape(fields[0]), null);
            if (id == null) return null;
            PadCustomButton b = new PadCustomButton(id);
            b.shape = PadCustomButton.Shape.valueOf(fields[1].trim().toUpperCase(Locale.ROOT));
            PadCustomButton.BindingKind kind =
                    PadCustomButton.BindingKind.valueOf(fields[2].trim().toUpperCase(Locale.ROOT));
            if (!applyBindingSpec(b, kind, fields[3])) return null;
            b.setCenter(Float.parseFloat(fields[4]), Float.parseFloat(fields[5]));
            b.setSize(Float.parseFloat(fields[6]), Float.parseFloat(fields[7]));
            b.setOpacity(Integer.parseInt(fields[8]));
            b.toggle = "1".equals(fields[9]);
            b.dragAim = "1".equals(fields[10]);
            if (fields.length >= 12) b.setLabel(unescape(fields[11]));
            return b;
        } catch (IllegalArgumentException e) {
            return null;
        }
    }

    private static String formatBindingSpec(PadCustomButton b) {
        switch (b.bindingKind) {
            case MOUSE_BUTTON:
                return Integer.toString(b.mouseButton);
            case MOUSE_SCROLL:
                return b.scrollDirection >= 0 ? "up" : "down";
            case KEY_CHORD: {
                int[] vks = b.keys();
                StringBuilder sb = new StringBuilder();
                for (int i = 0; i < vks.length; i++) {
                    if (i > 0) sb.append('+');
                    sb.append(String.format(Locale.ROOT, "%02X", vks[i] & 0xFF));
                }
                return sb.toString();
            }
            case KEY:
            default: {
                int[] vks = b.keys();
                return String.format(Locale.ROOT, "%02X", (vks.length > 0 ? vks[0] : 0x43) & 0xFF);
            }
        }
    }

    private static boolean applyBindingSpec(PadCustomButton b, PadCustomButton.BindingKind kind, String spec) {
        if (spec == null || spec.isEmpty()) return false;
        switch (kind) {
            case MOUSE_BUTTON:
                b.setMouseButton(Integer.parseInt(spec));
                return true;
            case MOUSE_SCROLL:
                if ("up".equalsIgnoreCase(spec) || "1".equals(spec)) b.setScrollDirection(1);
                else if ("down".equalsIgnoreCase(spec) || "-1".equals(spec)) b.setScrollDirection(-1);
                else return false;
                return true;
            case KEY_CHORD: {
                String[] parts = spec.split("\\+", -1);
                if (parts.length == 0 || parts.length > PadCustomButton.MAX_CHORD_KEYS) return false;
                int[] vks = new int[parts.length];
                for (int i = 0; i < parts.length; i++) vks[i] = Integer.parseInt(parts[i], 16);
                b.setChord(vks);
                return true;
            }
            case KEY:
            default:
                b.setKey(Integer.parseInt(spec, 16));
                return true;
        }
    }

    /** Percent-escapes delimiters and non-ASCII bytes for v1 pipe/comma safety. */
    static String escape(String text) {
        if (text == null || text.isEmpty()) return "";
        byte[] utf8 = text.getBytes(StandardCharsets.UTF_8);
        StringBuilder out = new StringBuilder(utf8.length);
        for (byte raw : utf8) {
            int c = raw & 0xFF;
            if ((c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9')
                    || c == '.' || c == '_' || c == '-' || c == ' ') {
                out.append((char) c);
            } else {
                out.append('%').append(Character.toUpperCase(Character.forDigit((c >> 4) & 0xF, 16)))
                        .append(Character.toUpperCase(Character.forDigit(c & 0xF, 16)));
            }
        }
        return out.toString();
    }

    static String unescape(String text) {
        if (text == null || text.isEmpty()) return "";
        byte[] out = new byte[text.length()];
        int len = 0;
        for (int i = 0; i < text.length(); i++) {
            char c = text.charAt(i);
            if (c == '%' && i + 2 < text.length()) {
                int hi = Character.digit(text.charAt(i + 1), 16);
                int lo = Character.digit(text.charAt(i + 2), 16);
                if (hi >= 0 && lo >= 0) {
                    out[len++] = (byte) ((hi << 4) | lo);
                    i += 2;
                    continue;
                }
            }
            if (c < 0x80) {
                out[len++] = (byte) c;
            } else {
                byte[] utf8 = String.valueOf(c).getBytes(StandardCharsets.UTF_8);
                if (len + utf8.length > out.length) {
                    byte[] grown = new byte[Math.max(out.length * 2, len + utf8.length)];
                    System.arraycopy(out, 0, grown, 0, len);
                    out = grown;
                }
                System.arraycopy(utf8, 0, out, len, utf8.length);
                len += utf8.length;
            }
        }
        return new String(out, 0, len, StandardCharsets.UTF_8);
    }

    /** Pure-JVM JSON serialization of touch zones and custom buttons for Hub Sync. */
    public static String toJson(PadLayout layout) {
        StringBuilder sb = new StringBuilder(256);
        sb.append("{\"version\":1,\"touch_zones\":[");
        List<PadTouchZone> zones = layout.touchZones();
        for (int i = 0; i < zones.size(); i++) {
            if (i > 0) sb.append(',');
            PadTouchZone z = zones.get(i);
            sb.append('{');
            jsonField(sb, "id", z.id).append(',');
            jsonField(sb, "kind", z.kind.name().toLowerCase(Locale.ROOT)).append(',');
            jsonFloat(sb, "left", z.left).append(',');
            jsonFloat(sb, "top", z.top).append(',');
            jsonFloat(sb, "right", z.right).append(',');
            jsonFloat(sb, "bottom", z.bottom).append(',');
            jsonInt(sb, "opacity", z.opacity).append(',');
            jsonBool(sb, "auto_sprint", z.autoSprint).append(',');
            jsonFloat(sb, "sprint_threshold", z.sprintThreshold).append(',');
            jsonFloat(sb, "deadzone", z.deadzone).append(',');
            jsonInt(sb, "sensitivity", z.sensitivityPercent).append(',');
            jsonBool(sb, "invert_y", z.invertY).append(',');
            jsonField(sb, "acceleration_curve", PadTouchZone.normalizeCurve(z.accelerationCurve)).append(',');
            jsonField(sb, "tap_action", z.tapAction.name().toLowerCase(Locale.ROOT)).append('}');
        }
        sb.append("],\"custom_buttons\":[");
        List<PadCustomButton> buttons = layout.customButtons();
        for (int i = 0; i < buttons.size(); i++) {
            if (i > 0) sb.append(',');
            PadCustomButton b = buttons.get(i);
            sb.append('{');
            jsonField(sb, "id", b.id).append(',');
            jsonField(sb, "label", b.label).append(',');
            jsonField(sb, "shape", b.shape.name().toLowerCase(Locale.ROOT)).append(',');
            jsonField(sb, "binding_kind", b.bindingKind.name().toLowerCase(Locale.ROOT)).append(',');
            sb.append("\"keys\":[");
            int[] vks = b.keys();
            for (int k = 0; k < vks.length; k++) {
                if (k > 0) sb.append(',');
                sb.append(vks[k]);
            }
            sb.append("],");
            jsonInt(sb, "mouse_button", b.mouseButton).append(',');
            jsonInt(sb, "scroll_direction", b.scrollDirection).append(',');
            jsonFloat(sb, "cx", b.cx).append(',');
            jsonFloat(sb, "cy", b.cy).append(',');
            jsonFloat(sb, "width_scale", b.widthScale).append(',');
            jsonFloat(sb, "height_scale", b.heightScale).append(',');
            jsonInt(sb, "opacity", b.opacity).append(',');
            jsonBool(sb, "toggle", b.toggle).append(',');
            jsonBool(sb, "drag_aim", b.dragAim).append('}');
        }
        sb.append("]}");
        return sb.toString();
    }

    /**
     * Applies zones and custom buttons from {@code json} onto {@code layout}. Invalid or null JSON
     * leaves {@code layout} unchanged.
     */
    public static boolean applyJson(PadLayout layout, String json) {
        if (layout == null || json == null || json.isEmpty()) return false;
        Object parsed;
        try {
            parsed = new JsonReader(json).parseRoot();
        } catch (IllegalArgumentException e) {
            return false;
        }
        if (!(parsed instanceof JsonMap)) return false;
        JsonMap root = (JsonMap) parsed;
        List<PadTouchZone> zones = new ArrayList<>();
        Set<String> zoneIds = new HashSet<>();
        Object rawZones = root.get("touch_zones");
        if (rawZones instanceof List<?>) {
            for (Object item : (List<?>) rawZones) {
                if (zones.size() >= PadTouchZone.MAX_ZONES) break;
                if (!(item instanceof JsonMap)) continue;
                PadTouchZone z = zoneFromJson((JsonMap) item);
                if (z != null && zoneIds.add(z.id)) zones.add(z);
            }
        }
        List<PadCustomButton> buttons = new ArrayList<>();
        Set<String> buttonIds = new HashSet<>();
        Object rawButtons = root.get("custom_buttons");
        if (rawButtons instanceof List<?>) {
            for (Object item : (List<?>) rawButtons) {
                if (buttons.size() >= PadCustomButton.MAX_CUSTOM_BUTTONS) break;
                if (!(item instanceof JsonMap)) continue;
                PadCustomButton b = buttonFromJson((JsonMap) item);
                if (b != null && buttonIds.add(b.id)) buttons.add(b);
            }
        }
        layout.clearTouchZones();
        for (PadTouchZone z : zones) layout.addTouchZone(z);
        layout.clearCustomButtons();
        for (PadCustomButton b : buttons) layout.addCustomButton(b);
        return true;
    }

    private static PadTouchZone zoneFromJson(JsonMap map) {
        try {
            String id = PadTouchZone.sanitizeId(map.str("id", ""), null);
            if (id == null) return null;
            PadTouchZone.Kind kind = PadTouchZone.Kind.valueOf(
                    map.str("kind", "look_mouse").toUpperCase(Locale.ROOT));
            PadTouchZone z = new PadTouchZone(id, kind, map.flt("left", 0.5f), map.flt("top", 0f),
                    map.flt("right", 1f), map.flt("bottom", 1f));
            z.setOpacity(map.num("opacity", PadTouchZone.DEFAULT_OPACITY));
            z.autoSprint = map.bool("auto_sprint", false);
            z.setSprintThreshold(map.flt("sprint_threshold", PadTouchZone.DEFAULT_SPRINT_THRESHOLD));
            z.setDeadzone(map.flt("deadzone", PadTouchZone.DEFAULT_DEADZONE));
            z.setSensitivityPercent(map.num("sensitivity", PadTouchZone.DEFAULT_SENSITIVITY));
            z.invertY = map.bool("invert_y", false);
            z.setAccelerationCurve(map.str("acceleration_curve", "linear").toLowerCase(Locale.ROOT));
            z.tapAction = PadTouchZone.TapAction.valueOf(
                    map.str("tap_action", "none").toUpperCase(Locale.ROOT));
            return z;
        } catch (IllegalArgumentException e) {
            return null;
        }
    }

    private static PadCustomButton buttonFromJson(JsonMap map) {
        try {
            String id = PadTouchZone.sanitizeId(map.str("id", ""), null);
            if (id == null) return null;
            PadCustomButton b = new PadCustomButton(id);
            b.setLabel(map.str("label", ""));
            b.shape = PadCustomButton.Shape.valueOf(map.str("shape", "round").toUpperCase(Locale.ROOT));
            PadCustomButton.BindingKind kind = PadCustomButton.BindingKind.valueOf(
                    map.str("binding_kind", "key").toUpperCase(Locale.ROOT));
            Object rawKeys = map.get("keys");
            int[] vks = extractVks(rawKeys);
            if (kind == PadCustomButton.BindingKind.MOUSE_BUTTON) {
                b.setMouseButton(map.num("mouse_button", 1));
            } else if (kind == PadCustomButton.BindingKind.MOUSE_SCROLL) {
                b.setScrollDirection(map.num("scroll_direction", 1));
            } else if (kind == PadCustomButton.BindingKind.KEY_CHORD) {
                b.setChord(vks);
            } else {
                b.setKey(vks.length > 0 ? vks[0] : 0x43);
            }
            b.setCenter(map.flt("cx", 0.5f), map.flt("cy", 0.5f));
            b.setSize(map.flt("width_scale", 1f), map.flt("height_scale", 1f));
            b.setOpacity(map.num("opacity", PadCustomButton.DEFAULT_OPACITY));
            b.toggle = map.bool("toggle", false);
            b.dragAim = map.bool("drag_aim", false);
            return b;
        } catch (IllegalArgumentException e) {
            return null;
        }
    }

    private static int[] extractVks(Object rawKeys) {
        if (!(rawKeys instanceof List<?>)) return new int[] {0x43};
        List<?> list = (List<?>) rawKeys;
        int[] out = new int[Math.min(PadCustomButton.MAX_CHORD_KEYS, list.size())];
        int count = 0;
        for (Object item : list) {
            if (count >= out.length) break;
            if (item instanceof Number) out[count++] = ((Number) item).intValue();
        }
        return count == 0 ? new int[] {0x43} : java.util.Arrays.copyOf(out, count);
    }

    private static StringBuilder jsonField(StringBuilder sb, String k, String v) {
        jsonKey(sb, k);
        return jsonString(sb, v == null ? "" : v);
    }

    private static StringBuilder jsonInt(StringBuilder sb, String k, int v) {
        return jsonKey(sb, k).append(v);
    }

    private static StringBuilder jsonFloat(StringBuilder sb, String k, float v) {
        return jsonKey(sb, k).append(String.format(Locale.ROOT, "%.4f", Float.isFinite(v) ? v : 0f));
    }

    private static StringBuilder jsonBool(StringBuilder sb, String k, boolean v) {
        return jsonKey(sb, k).append(v ? "true" : "false");
    }

    private static StringBuilder jsonKey(StringBuilder sb, String k) {
        return jsonString(sb, k).append(':');
    }

    private static StringBuilder jsonString(StringBuilder sb, String v) {
        sb.append('"');
        for (int i = 0; i < v.length(); i++) {
            char c = v.charAt(i);
            if (c == '"' || c == '\\') sb.append('\\').append(c);
            else if (c < 0x20) sb.append(String.format(Locale.ROOT, "\\u%04x", (int) c));
            else sb.append(c);
        }
        return sb.append('"');
    }

    private static final class JsonMap {
        final List<String> keys = new ArrayList<>();
        final List<Object> values = new ArrayList<>();

        void put(String key, Object value) {
            keys.add(key);
            values.add(value);
        }

        Object get(String key) {
            for (int i = keys.size() - 1; i >= 0; i--) {
                if (keys.get(i).equals(key)) return values.get(i);
            }
            return null;
        }

        String str(String key, String def) {
            Object v = get(key);
            return v instanceof String ? (String) v : def;
        }

        int num(String key, int def) {
            Object v = get(key);
            return v instanceof Number ? ((Number) v).intValue() : def;
        }

        float flt(String key, float def) {
            Object v = get(key);
            return v instanceof Number ? ((Number) v).floatValue() : def;
        }

        boolean bool(String key, boolean def) {
            Object v = get(key);
            return v instanceof Boolean ? (Boolean) v : def;
        }
    }

    private static final class JsonReader {
        private final String s;
        private int pos;

        JsonReader(String s) {
            this.s = s;
        }

        Object parseRoot() {
            skipWs();
            Object v = parseValue();
            skipWs();
            if (pos != s.length()) throw new IllegalArgumentException("Trailing JSON content");
            return v;
        }

        private Object parseValue() {
            skipWs();
            if (pos >= s.length()) throw new IllegalArgumentException("Unexpected end");
            char c = s.charAt(pos);
            if (c == '{') return parseObject();
            if (c == '[') return parseArray();
            if (c == '"') return parseString();
            if (c == 't' && s.startsWith("true", pos)) { pos += 4; return Boolean.TRUE; }
            if (c == 'f' && s.startsWith("false", pos)) { pos += 5; return Boolean.FALSE; }
            if (c == 'n' && s.startsWith("null", pos)) { pos += 4; return null; }
            return parseNumber();
        }

        private JsonMap parseObject() {
            pos++;
            JsonMap map = new JsonMap();
            skipWs();
            if (pos < s.length() && s.charAt(pos) == '}') { pos++; return map; }
            while (true) {
                skipWs();
                if (pos >= s.length() || s.charAt(pos) != '"') throw new IllegalArgumentException("Expected key");
                String key = parseString();
                skipWs();
                if (pos >= s.length() || s.charAt(pos) != ':') throw new IllegalArgumentException("Expected ':'");
                pos++;
                map.put(key, parseValue());
                skipWs();
                if (pos >= s.length()) throw new IllegalArgumentException("Unterminated object");
                char next = s.charAt(pos++);
                if (next == '}') return map;
                if (next != ',') throw new IllegalArgumentException("Expected ',' or '}'");
            }
        }

        private List<Object> parseArray() {
            pos++;
            List<Object> list = new ArrayList<>();
            skipWs();
            if (pos < s.length() && s.charAt(pos) == ']') { pos++; return list; }
            while (true) {
                list.add(parseValue());
                skipWs();
                if (pos >= s.length()) throw new IllegalArgumentException("Unterminated array");
                char next = s.charAt(pos++);
                if (next == ']') return list;
                if (next != ',') throw new IllegalArgumentException("Expected ',' or ']'");
            }
        }

        private String parseString() {
            pos++;
            StringBuilder out = new StringBuilder();
            while (pos < s.length()) {
                char c = s.charAt(pos++);
                if (c == '"') return out.toString();
                if (c == '\\') {
                    if (pos >= s.length()) throw new IllegalArgumentException("Bad escape");
                    char esc = s.charAt(pos++);
                    switch (esc) {
                        case '"': case '\\': case '/': out.append(esc); break;
                        case 'b': out.append('\b'); break;
                        case 'f': out.append('\f'); break;
                        case 'n': out.append('\n'); break;
                        case 'r': out.append('\r'); break;
                        case 't': out.append('\t'); break;
                        case 'u':
                            if (pos + 4 > s.length()) throw new IllegalArgumentException("Bad unicode");
                            out.append((char) Integer.parseInt(s.substring(pos, pos + 4), 16));
                            pos += 4;
                            break;
                        default: throw new IllegalArgumentException("Bad escape");
                    }
                } else {
                    out.append(c);
                }
            }
            throw new IllegalArgumentException("Unterminated string");
        }

        private Number parseNumber() {
            int start = pos;
            while (pos < s.length()) {
                char c = s.charAt(pos);
                if ((c >= '0' && c <= '9') || c == '-' || c == '+' || c == '.' || c == 'e' || c == 'E') pos++;
                else break;
            }
            if (pos == start) throw new IllegalArgumentException("Expected number");
            try {
                return Double.valueOf(s.substring(start, pos));
            } catch (NumberFormatException e) {
                throw new IllegalArgumentException("Bad number", e);
            }
        }

        private void skipWs() {
            while (pos < s.length() && Character.isWhitespace(s.charAt(pos))) pos++;
        }
    }
}

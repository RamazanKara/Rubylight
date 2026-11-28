package com.limelight.binding.input.pad;

import com.limelight.nvstream.input.MouseButtonPacket;

import java.util.Arrays;
import java.util.Locale;

/**
 * Pure-JVM data model for an addable, duplicable, resizable, and stylable custom keyboard/mouse
 * on-screen button: supports single Windows {@code VK_*} keys, up to 4-key chords, mouse buttons,
 * and mouse wheel scroll actions, with Hold/Toggle and Aim-while-holding modes and {@code >= 48dp} bounds.
 */
public final class PadCustomButton {
    public static final int MAX_CUSTOM_BUTTONS = 24;
    public static final int MAX_CHORD_KEYS = 4;
    public static final int MAX_LABEL_LENGTH = 16;
    public static final int MIN_OPACITY = 0, MAX_OPACITY = 100, DEFAULT_OPACITY = 90;
    public static final float MIN_SCALE = 0.5f, MAX_SCALE = 100f;
    public static final float MIN_TOUCH_DP = 48f;
    public static final float BASE_ROUND_DP = 52f;
    public static final float BASE_PILL_W_DP = 84f, BASE_PILL_H_DP = 52f;
    public static final float BASE_RECT_W_DP = 68f, BASE_RECT_H_DP = 56f;

    public enum Shape { ROUND, PILL, RECT }

    public enum BindingKind { KEY, KEY_CHORD, MOUSE_BUTTON, MOUSE_SCROLL }

    public String id;
    public String label = "";
    public Shape shape = Shape.ROUND;
    public BindingKind bindingKind = BindingKind.KEY;
    private int[] keys = {0x43};
    public int mouseButton = MouseButtonPacket.BUTTON_LEFT;
    public int scrollDirection = 1;
    public float cx = 0.50f;
    public float cy = 0.50f;
    public float widthScale = 1.0f;
    public float heightScale = 1.0f;
    public int opacity = DEFAULT_OPACITY;
    public boolean toggle;
    public boolean dragAim;

    public PadCustomButton() {
        this("kbm1");
    }

    public PadCustomButton(String id) {
        this.id = PadTouchZone.sanitizeId(id, "kbm1");
    }

    public static PadCustomButton key(String id, int vk, String label, float cx, float cy) {
        PadCustomButton button = new PadCustomButton(id);
        button.setKey(vk);
        button.setLabel(label);
        button.setCenter(cx, cy);
        return button;
    }

    public static PadCustomButton chord(String id, int[] vks, String label, float cx, float cy) {
        PadCustomButton button = new PadCustomButton(id);
        button.setChord(vks);
        button.setLabel(label);
        button.setCenter(cx, cy);
        return button;
    }

    public static PadCustomButton mouse(String id, int mouseButton, String label, float cx, float cy) {
        PadCustomButton button = new PadCustomButton(id);
        button.setMouseButton(mouseButton);
        button.setLabel(label);
        button.setCenter(cx, cy);
        return button;
    }

    public static PadCustomButton scroll(String id, int direction, String label, float cx, float cy) {
        PadCustomButton button = new PadCustomButton(id);
        button.setScrollDirection(direction);
        button.setLabel(label);
        button.setCenter(cx, cy);
        return button;
    }

    public void setKey(int vk) {
        bindingKind = BindingKind.KEY;
        keys = new int[] {clampVk(vk)};
    }

    public void setChord(int[] vks) {
        bindingKind = BindingKind.KEY_CHORD;
        if (vks == null || vks.length == 0) {
            keys = new int[] {0x43};
            return;
        }
        int[] unique = new int[Math.min(MAX_CHORD_KEYS, vks.length)];
        int count = 0;
        for (int raw : vks) {
            if (raw < 1 || raw > 0xFF) continue;
            boolean dup = false;
            for (int i = 0; i < count; i++) {
                if (unique[i] == raw) {
                    dup = true;
                    break;
                }
            }
            if (!dup) {
                unique[count++] = raw;
                if (count == MAX_CHORD_KEYS) break;
            }
        }
        if (count == 0) {
            keys = new int[] {0x43};
        } else {
            // Put modifier VKs first so press presses modifiers first and release releases them last.
            int[] ordered = new int[count];
            int at = 0;
            for (int i = 0; i < count; i++) if (PadHeldInputs.isModifierVk(unique[i])) ordered[at++] = unique[i];
            for (int i = 0; i < count; i++) if (!PadHeldInputs.isModifierVk(unique[i])) ordered[at++] = unique[i];
            keys = ordered;
        }
    }

    public void setMouseButton(int button) {
        bindingKind = BindingKind.MOUSE_BUTTON;
        mouseButton = clampMouseButton(button);
    }

    public void setScrollDirection(int direction) {
        bindingKind = BindingKind.MOUSE_SCROLL;
        scrollDirection = direction < 0 ? -1 : 1;
    }

    public int[] keys() {
        return keys.clone();
    }

    /** Zero-allocation read access for play-mode binding dispatch. */
    int keyAt(int index) {
        return keys[index];
    }

    int keyCount() {
        return keys.length;
    }

    public void setLabel(String label) {
        this.label = sanitizeLabel(label);
    }

    public String effectiveLabel() {
        if (label != null && !label.isEmpty()) return label;
        return defaultBindingLabel();
    }

    public String defaultBindingLabel() {
        switch (bindingKind) {
            case MOUSE_BUTTON:
                return mouseButtonLabel(mouseButton);
            case MOUSE_SCROLL:
                return scrollDirection >= 0 ? "SCR+" : "SCR-";
            case KEY_CHORD: {
                StringBuilder sb = new StringBuilder();
                for (int i = 0; i < keys.length; i++) {
                    if (i > 0) sb.append('+');
                    sb.append(vkShortLabel(keys[i]));
                }
                String joined = sb.toString();
                return joined.length() <= MAX_LABEL_LENGTH ? joined : joined.substring(0, MAX_LABEL_LENGTH);
            }
            case KEY:
            default:
                return vkShortLabel(keys.length > 0 ? keys[0] : 0x43);
        }
    }

    public void setCenter(float cx, float cy) {
        this.cx = clamp(finiteOr(cx, 0.5f), 0f, 1f);
        this.cy = clamp(finiteOr(cy, 0.5f), 0f, 1f);
    }

    public void setScale(float scale) {
        setSize(scale, scale);
    }

    public void setSize(float widthScale, float heightScale) {
        this.widthScale = clampScale(widthScale);
        this.heightScale = clampScale(heightScale);
    }

    public float scale() {
        return Math.max(widthScale, heightScale);
    }

    public void setOpacity(int opacity) {
        this.opacity = Math.max(MIN_OPACITY, Math.min(MAX_OPACITY, opacity));
    }

    /** Computes {left, top, width, height} in pixels inside a safe surface of {@code width} x {@code height}. */
    public float[] bounds(float width, float height, float density, float[] out) {
        float d = Math.max(1f, density);
        float minPx = MIN_TOUCH_DP * d;
        float bw = Math.max(baseWidthDp(shape) * d * widthScale, minPx);
        float bh = Math.max(baseHeightDp(shape) * d * heightScale, minPx);
        float w = Math.min(bw, Math.max(0f, width));
        float h = Math.min(bh, Math.max(0f, height));
        out[0] = clamp(cx * width - w / 2f, 0f, Math.max(0f, width - w));
        out[1] = clamp(cy * height - h / 2f, 0f, Math.max(0f, height - h));
        out[2] = w;
        out[3] = h;
        return out;
    }

    public void moveTo(float centerX, float centerY, float surfaceWidth, float surfaceHeight, float density) {
        float[] box = bounds(surfaceWidth, surfaceHeight, density, new float[4]);
        float halfW = box[2] / 2f;
        float halfH = box[3] / 2f;
        cx = clamp(centerX, halfW, Math.max(halfW, surfaceWidth - halfW)) / Math.max(1f, surfaceWidth);
        cy = clamp(centerY, halfH, Math.max(halfH, surfaceHeight - halfH)) / Math.max(1f, surfaceHeight);
    }

    public void setBounds(float left, float top, float w, float h, float surfaceWidth, float surfaceHeight, float density) {
        float d = Math.max(1f, density);
        float bw = Math.max(1e-3f, baseWidthDp(shape) * d);
        float bh = Math.max(1e-3f, baseHeightDp(shape) * d);
        setSize(Math.min(w, surfaceWidth) / bw, Math.min(h, surfaceHeight) / bh);
        moveTo(left + w / 2f, top + h / 2f, surfaceWidth, surfaceHeight, density);
    }

    public void scaleFrom(float startWidthScale, float startHeightScale, float factor,
                          float surfaceWidth, float surfaceHeight, float density) {
        float d = Math.max(1f, density);
        float least = Math.max(MIN_SCALE / Math.max(1e-6f, startWidthScale),
                MIN_SCALE / Math.max(1e-6f, startHeightScale));
        float most = Float.MAX_VALUE;
        if (surfaceWidth > 0 && surfaceHeight > 0) {
            most = Math.min(surfaceWidth / Math.max(1e-6f, baseWidthDp(shape) * d * startWidthScale),
                    surfaceHeight / Math.max(1e-6f, baseHeightDp(shape) * d * startHeightScale));
        }
        float f = Float.isNaN(factor) ? 1f : factor;
        f = most >= least ? Math.max(least, Math.min(most, f)) : Math.min(f, 1f);
        setSize(startWidthScale * f, startHeightScale * f);
    }

    public PadCustomButton duplicate(String newId) {
        PadCustomButton dup = copy();
        dup.id = PadTouchZone.sanitizeId(newId, id + "_2");
        dup.cx = cx <= 0.85f ? cx + 0.06f : cx - 0.06f;
        dup.cy = cy <= 0.85f ? cy + 0.06f : cy - 0.06f;
        return dup;
    }

    public PadCustomButton copy() {
        PadCustomButton copy = new PadCustomButton(id);
        copy.copyFrom(this);
        return copy;
    }

    public void copyFrom(PadCustomButton other) {
        if (other == null) return;
        id = PadTouchZone.sanitizeId(other.id, "kbm1");
        label = sanitizeLabel(other.label);
        shape = other.shape == null ? Shape.ROUND : other.shape;
        bindingKind = other.bindingKind == null ? BindingKind.KEY : other.bindingKind;
        keys = other.keys == null || other.keys.length == 0 ? new int[] {0x43} : other.keys.clone();
        mouseButton = clampMouseButton(other.mouseButton);
        scrollDirection = other.scrollDirection < 0 ? -1 : 1;
        setCenter(other.cx, other.cy);
        setSize(other.widthScale, other.heightScale);
        setOpacity(other.opacity);
        toggle = other.toggle;
        dragAim = other.dragAim;
    }

    public static float baseWidthDp(Shape shape) {
        if (shape == Shape.PILL) return BASE_PILL_W_DP;
        if (shape == Shape.RECT) return BASE_RECT_W_DP;
        return BASE_ROUND_DP;
    }

    public static float baseHeightDp(Shape shape) {
        if (shape == Shape.PILL) return BASE_PILL_H_DP;
        if (shape == Shape.RECT) return BASE_RECT_H_DP;
        return BASE_ROUND_DP;
    }

    public static String mouseButtonLabel(int button) {
        switch (button) {
            case MouseButtonPacket.BUTTON_LEFT: return "LMB";
            case MouseButtonPacket.BUTTON_RIGHT: return "RMB";
            case MouseButtonPacket.BUTTON_MIDDLE: return "MMB";
            case MouseButtonPacket.BUTTON_X1: return "X1";
            case MouseButtonPacket.BUTTON_X2: return "X2";
            default: return "M" + button;
        }
    }

    public static String vkShortLabel(int vk) {
        if (vk >= 0x41 && vk <= 0x5A) return String.valueOf((char) vk);
        if (vk >= 0x30 && vk <= 0x39) return String.valueOf((char) vk);
        if (vk >= 0x70 && vk <= 0x87) return "F" + (vk - 0x70 + 1);
        if (vk >= 0x60 && vk <= 0x69) return "N" + (vk - 0x60);
        switch (vk) {
            case 0x08: return "BKSP";
            case 0x09: return "TAB";
            case 0x0D: return "ENT";
            case 0x13: return "PAUSE";
            case 0x14: return "CAPS";
            case 0x1B: return "ESC";
            case 0x20: return "SPACE";
            case 0x21: return "PGUP";
            case 0x22: return "PGDN";
            case 0x23: return "END";
            case 0x24: return "HOME";
            case 0x25: return "LEFT";
            case 0x26: return "UP";
            case 0x27: return "RIGHT";
            case 0x28: return "DOWN";
            case 0x2C: return "PRTSC";
            case 0x2D: return "INS";
            case 0x2E: return "DEL";
            case 0x5B: case 0x5C: return "WIN";
            case 0x5D: return "MENU";
            case 0x90: return "NUMLK";
            case 0x91: return "SCRLK";
            case 0xA0: case 0xA1: return "SHIFT";
            case 0xA2: case 0xA3: return "CTRL";
            case 0xA4: case 0xA5: return "ALT";
            case 0xBA: return ";";
            case 0xBB: return "=";
            case 0xBC: return ",";
            case 0xBD: return "-";
            case 0xBE: return ".";
            case 0xBF: return "/";
            case 0xC0: return "`";
            case 0xDB: return "[";
            case 0xDC: return "\\";
            case 0xDD: return "]";
            case 0xDE: return "'";
            default: return String.format(Locale.ROOT, "0x%02X", vk & 0xFF);
        }
    }

    static int clampVk(int vk) {
        return vk < 1 || vk > 0xFF ? 0x43 : vk;
    }

    static int clampMouseButton(int button) {
        return button < MouseButtonPacket.BUTTON_LEFT || button > MouseButtonPacket.BUTTON_X2
                ? MouseButtonPacket.BUTTON_LEFT : button;
    }

    static float clampScale(float scale) {
        return Float.isNaN(scale) ? 1f : clamp(scale, MIN_SCALE, MAX_SCALE);
    }

    static String sanitizeLabel(String raw) {
        if (raw == null) return "";
        StringBuilder out = new StringBuilder(Math.min(MAX_LABEL_LENGTH, raw.length()));
        for (int i = 0; i < raw.length(); i++) {
            char c = raw.charAt(i);
            if (!Character.isISOControl(c)) out.append(c);
        }
        String trimmed = out.toString().trim();
        return trimmed.length() <= MAX_LABEL_LENGTH ? trimmed : trimmed.substring(0, MAX_LABEL_LENGTH);
    }

    private static float finiteOr(float value, float fallback) {
        return Float.isNaN(value) || Float.isInfinite(value) ? fallback : value;
    }

    private static float clamp(float value, float min, float max) {
        return Math.max(min, Math.min(max, value));
    }

    @Override
    public String toString() {
        return "PadCustomButton{" + id + "," + bindingKind + "," + Arrays.toString(keys) + "}";
    }
}

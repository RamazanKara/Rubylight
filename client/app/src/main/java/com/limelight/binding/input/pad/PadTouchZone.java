package com.limelight.binding.input.pad;

import com.limelight.binding.input.touch.PointerAcceleration;

import java.util.List;

/**
 * Pure-JVM data model for a full- or partial-screen mobile FPS touch zone: floating movement
 * (WASD + auto-sprint or analog stick) or floating camera look (relative mouse or analog stick)
 * with normalized edge-to-edge bounds, stealth opacity (0 by default), sensitivity, Y inversion,
 * acceleration curve, tap action, and quick split presets.
 */
public final class PadTouchZone {
    public static final int MAX_ZONES = 4;
    public static final float MIN_SIZE = 0.10f;
    public static final int MIN_OPACITY = 0, MAX_OPACITY = 100, DEFAULT_OPACITY = 0;
    public static final int MIN_SENSITIVITY = 25, MAX_SENSITIVITY = 400, DEFAULT_SENSITIVITY = 100;
    public static final float MIN_DEADZONE = 0f, MAX_DEADZONE = 0.90f, DEFAULT_DEADZONE = 0.08f;
    public static final float MIN_SPRINT_THRESHOLD = 0.50f, MAX_SPRINT_THRESHOLD = 1.00f;
    public static final float DEFAULT_SPRINT_THRESHOLD = 0.85f;

    public enum Kind {
        MOVE_WASD, MOVE_STICK, LOOK_MOUSE, LOOK_STICK;

        public boolean isMove() {
            return this == MOVE_WASD || this == MOVE_STICK;
        }

        public boolean isLook() {
            return this == LOOK_MOUSE || this == LOOK_STICK;
        }
    }

    public enum TapAction { NONE, LEFT_CLICK, RIGHT_CLICK }

    public enum QuickPreset { SPLIT_50_50, SPLIT_40_60, RIGHT_CAMERA_ONLY, CUSTOM_BOX }

    public String id;
    public Kind kind = Kind.LOOK_MOUSE;
    public float left = 0.50f;
    public float top = 0.00f;
    public float right = 1.00f;
    public float bottom = 1.00f;
    public int opacity = DEFAULT_OPACITY;
    public boolean autoSprint;
    public float sprintThreshold = DEFAULT_SPRINT_THRESHOLD;
    public float deadzone = DEFAULT_DEADZONE;
    public int sensitivityPercent = DEFAULT_SENSITIVITY;
    public boolean invertY;
    public String accelerationCurve = PointerAcceleration.LINEAR;
    public TapAction tapAction = TapAction.NONE;

    public PadTouchZone() {
        this("zone1", Kind.LOOK_MOUSE, 0.50f, 0.00f, 1.00f, 1.00f);
    }

    public PadTouchZone(String id, Kind kind, float left, float top, float right, float bottom) {
        this.id = sanitizeId(id, "zone1");
        this.kind = kind == null ? Kind.LOOK_MOUSE : kind;
        setBounds(left, top, right, bottom);
    }

    /** Sets normalized bounds in [0, 1], enforcing at least {@link #MIN_SIZE} in each dimension. */
    public void setBounds(float left, float top, float right, float bottom) {
        float l = finiteOr(left, 0f);
        float t = finiteOr(top, 0f);
        float r = finiteOr(right, l + MIN_SIZE);
        float b = finiteOr(bottom, t + MIN_SIZE);
        if (l > r) {
            float tmp = l;
            l = r;
            r = tmp;
        }
        if (t > b) {
            float tmp = t;
            t = b;
            b = tmp;
        }
        l = clamp(l, 0f, 1f - MIN_SIZE);
        t = clamp(t, 0f, 1f - MIN_SIZE);
        r = clamp(r, l + MIN_SIZE, 1f);
        b = clamp(b, t + MIN_SIZE, 1f);
        this.left = l;
        this.top = t;
        this.right = r;
        this.bottom = b;
    }

    /** Moves this zone to normalized center ({@code cx}, {@code cy}) preserving its width and height. */
    public void moveTo(float cx, float cy) {
        float w = right - left;
        float h = bottom - top;
        float nx = clamp(finiteOr(cx, 0.5f) - w / 2f, 0f, Math.max(0f, 1f - w));
        float ny = clamp(finiteOr(cy, 0.5f) - h / 2f, 0f, Math.max(0f, 1f - h));
        setBounds(nx, ny, nx + w, ny + h);
    }

    /** Writes pixel box {left, top, width, height} for a surface of {@code width} x {@code height}. */
    public float[] pixelBounds(float width, float height, float[] out) {
        float w = Math.max(0f, width);
        float h = Math.max(0f, height);
        out[0] = left * w;
        out[1] = top * h;
        out[2] = Math.max(0f, (right - left) * w);
        out[3] = Math.max(0f, (bottom - top) * h);
        return out;
    }

    public void setOpacity(int opacity) {
        this.opacity = Math.max(MIN_OPACITY, Math.min(MAX_OPACITY, opacity));
    }

    public void setSensitivityPercent(int percent) {
        this.sensitivityPercent = Math.max(MIN_SENSITIVITY, Math.min(MAX_SENSITIVITY, percent));
    }

    public void setDeadzone(float deadzone) {
        this.deadzone = clamp(finiteOr(deadzone, DEFAULT_DEADZONE), MIN_DEADZONE, MAX_DEADZONE);
    }

    public void setSprintThreshold(float threshold) {
        this.sprintThreshold = clamp(finiteOr(threshold, DEFAULT_SPRINT_THRESHOLD),
                MIN_SPRINT_THRESHOLD, MAX_SPRINT_THRESHOLD);
    }

    public void setAccelerationCurve(String curve) {
        this.accelerationCurve = normalizeCurve(curve);
    }

    public PadTouchZone copy() {
        PadTouchZone copy = new PadTouchZone();
        copy.copyFrom(this);
        return copy;
    }

    public void copyFrom(PadTouchZone other) {
        if (other == null) return;
        id = sanitizeId(other.id, "zone1");
        kind = other.kind == null ? Kind.LOOK_MOUSE : other.kind;
        setBounds(other.left, other.top, other.right, other.bottom);
        setOpacity(other.opacity);
        autoSprint = other.autoSprint;
        setSprintThreshold(other.sprintThreshold);
        setDeadzone(other.deadzone);
        setSensitivityPercent(other.sensitivityPercent);
        invertY = other.invertY;
        setAccelerationCurve(other.accelerationCurve);
        tapAction = other.tapAction == null ? TapAction.NONE : other.tapAction;
    }

    /** Replaces {@code out} with the zones of {@code preset}. */
    public static void applyQuickPreset(List<PadTouchZone> out, QuickPreset preset) {
        applyQuickPreset(out, preset, false);
    }

    /** Replaces {@code out} with the zones of {@code preset}, using gamepad sticks when {@code gamepad} is true. */
    public static void applyQuickPreset(List<PadTouchZone> out, QuickPreset preset, boolean gamepad) {
        if (out == null || preset == null) return;
        out.clear();
        Kind moveKind = gamepad ? Kind.MOVE_STICK : Kind.MOVE_WASD;
        Kind lookKind = gamepad ? Kind.LOOK_STICK : Kind.LOOK_MOUSE;
        switch (preset) {
            case SPLIT_50_50: {
                PadTouchZone move = new PadTouchZone("move", moveKind, 0f, 0f, 0.50f, 1f);
                move.autoSprint = !gamepad;
                out.add(move);
                out.add(new PadTouchZone("look", lookKind, 0.50f, 0f, 1f, 1f));
                break;
            }
            case SPLIT_40_60: {
                PadTouchZone move = new PadTouchZone("move", moveKind, 0f, 0f, 0.40f, 1f);
                move.autoSprint = !gamepad;
                out.add(move);
                out.add(new PadTouchZone("look", lookKind, 0.40f, 0f, 1f, 1f));
                break;
            }
            case RIGHT_CAMERA_ONLY:
                out.add(new PadTouchZone("look", lookKind, 0.50f, 0f, 1f, 1f));
                break;
            case CUSTOM_BOX:
            default:
                out.add(new PadTouchZone("zone1", lookKind, 0.55f, 0.25f, 0.92f, 0.78f));
                break;
        }
    }

    public static String normalizeCurve(String curve) {
        if (PointerAcceleration.GENTLE.equals(curve)) return PointerAcceleration.GENTLE;
        if (PointerAcceleration.MEDIUM.equals(curve)) return PointerAcceleration.MEDIUM;
        if (PointerAcceleration.STRONG.equals(curve)) return PointerAcceleration.STRONG;
        return PointerAcceleration.LINEAR;
    }

    static String sanitizeId(String raw, String fallback) {
        if (raw != null) {
            String trimmed = raw.trim();
            if (!trimmed.isEmpty()) {
                StringBuilder clean = new StringBuilder(Math.min(32, trimmed.length()));
                for (int i = 0; i < trimmed.length() && clean.length() < 32; i++) {
                    char c = trimmed.charAt(i);
                    if ((c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z')
                            || (c >= '0' && c <= '9') || c == '_' || c == '-') {
                        clean.append(c);
                    }
                }
                if (clean.length() > 0) return clean.toString();
            }
        }
        return fallback;
    }

    private static float finiteOr(float value, float fallback) {
        return Float.isNaN(value) || Float.isInfinite(value) ? fallback : value;
    }

    private static float clamp(float value, float min, float max) {
        return Math.max(min, Math.min(max, value));
    }
}

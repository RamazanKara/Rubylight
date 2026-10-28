package com.limelight.binding.input.pad;

import java.util.Locale;

/**
 * Where the modern on-screen controller's controls sit, how big they are, how opaque, and which are
 * hidden. A control the user never moved follows the default for the current surface, so a layout
 * nobody edited still fits after a rotation; a moved control keeps its center as a fraction of the
 * surface. Plain data and math, so persistence and geometry are tested without a device.
 */
public final class PadLayout {
    /** The controls, in drawing order. Shapes: round sticks and buttons, pill shoulders and triggers. */
    public enum Control {
        LEFT_STICK(Shape.STICK), RIGHT_STICK(Shape.STICK), DPAD(Shape.DPAD), FACE(Shape.FACE),
        LB(Shape.PILL), RB(Shape.PILL), LT(Shape.PILL), RT(Shape.PILL),
        L3(Shape.ROUND), R3(Shape.ROUND), SELECT(Shape.ROUND), START(Shape.ROUND), HOME(Shape.ROUND);

        public final Shape shape;

        Control(Shape shape) {
            this.shape = shape;
        }

        static Control named(String name) {
            for (Control control : values()) {
                if (control.name().equals(name)) return control;
            }
            return null;
        }
    }

    public enum Shape { STICK, DPAD, FACE, PILL, ROUND }

    public static final float MIN_SCALE = .5f, MAX_SCALE = 2f;
    public static final int MIN_OPACITY = 10, MAX_OPACITY = 100;
    /** The longest side of the default unit: a tablet's controls stop growing past a large phone's. */
    static final float MAX_UNIT_DP = 440;
    private static final String VERSION = "v1";

    /** One control's settings. {@code cx} and {@code cy} are fractions of the surface; NaN means the default place. */
    public static final class Spec {
        public float cx = Float.NaN, cy = Float.NaN, scale = 1;
        public int opacity = MAX_OPACITY;
        public boolean hidden;

        public boolean placed() {
            return !Float.isNaN(cx) && !Float.isNaN(cy);
        }

        void copyFrom(Spec other) {
            cx = other.cx; cy = other.cy; scale = other.scale; opacity = other.opacity; hidden = other.hidden;
        }

        boolean isDefault() {
            return !placed() && scale == 1 && opacity == MAX_OPACITY && !hidden;
        }
    }

    private final Spec[] specs = new Spec[Control.values().length];

    public PadLayout() {
        for (int i = 0; i < specs.length; i++) specs[i] = new Spec();
    }

    public Spec spec(Control control) {
        return specs[control.ordinal()];
    }

    public PadLayout copy() {
        PadLayout copy = new PadLayout();
        for (int i = 0; i < specs.length; i++) copy.specs[i].copyFrom(specs[i]);
        return copy;
    }

    public void copyFrom(PadLayout other) {
        for (int i = 0; i < specs.length; i++) specs[i].copyFrom(other.specs[i]);
    }

    public boolean isDefault() {
        for (Spec spec : specs) {
            if (!spec.isDefault()) return false;
        }
        return true;
    }

    public void reset() {
        for (Spec spec : specs) spec.copyFrom(new Spec());
    }

    public void setScale(Control control, float scale) {
        spec(control).scale = clamp(scale, MIN_SCALE, MAX_SCALE);
    }

    public void setOpacity(Control control, int opacity) {
        spec(control).opacity = Math.max(MIN_OPACITY, Math.min(MAX_OPACITY, opacity));
    }

    /** Puts the control's center at ({@code x}, {@code y}) pixels, kept so the whole control stays on the surface. */
    public void moveTo(Control control, float x, float y, float width, float height, float density) {
        float[] box = bounds(control, width, height, density, new float[4]);
        float halfW = box[2] / 2, halfH = box[3] / 2;
        Spec spec = spec(control);
        spec.cx = clamp(x, halfW, Math.max(halfW, width - halfW)) / Math.max(1, width);
        spec.cy = clamp(y, halfH, Math.max(halfH, height - halfH)) / Math.max(1, height);
    }

    /**
     * The default size unit: the short side of the surface, capped for tablets, and narrow enough
     * that the left and right clusters (about 1.75 units together) never meet on a portrait phone.
     */
    static float unit(float width, float height, float density) {
        return Math.min(Math.min(Math.min(width, height), MAX_UNIT_DP * density), width / 1.75f);
    }

    /** Width and height of a control at scale 1, in units. */
    static float baseWidth(Control control) {
        switch (control.shape) {
            case STICK: return control == Control.LEFT_STICK ? .42f : .34f;
            case DPAD: return .30f;
            case FACE: return .40f;
            case PILL: return .25f;
            default: return control == Control.L3 || control == Control.R3 ? .13f : .11f;
        }
    }

    static float baseHeight(Control control) {
        return control.shape == Shape.PILL ? .12f : baseWidth(control);
    }

    /**
     * Width and height of a control in pixels at {@code scale}: never below a 48 dp touch, and the
     * pill shoulders and triggers never narrower than 72 dp, whatever the surface or the scale.
     */
    static float width(Control control, float u, float density, float scale) {
        return Math.max(baseWidth(control) * u * scale,
                (control.shape == Shape.PILL ? MIN_PILL_WIDTH_DP : MIN_TOUCH_DP) * density);
    }

    static float height(Control control, float u, float density, float scale) {
        return Math.max(baseHeight(control) * u * scale, MIN_TOUCH_DP * density);
    }

    /** The smallest touch a control offers, and the narrowest a pill gets so it still reads as one. */
    static final float MIN_TOUCH_DP = 48, MIN_PILL_WIDTH_DP = 72;

    /**
     * The default center of a control, in pixels, from the nearest edges of the surface. Edge controls
     * keep their margins from their own size, so a control grown to its minimum touch on a small
     * surface still clears its neighbours: the shoulder pair, the top row and the stick clicks below
     * the clusters.
     */
    static void defaultCenter(Control control, float width, float height, float u, float density, float[] out) {
        float pillW = width(Control.LT, u, density, 1), pillH = height(Control.LT, u, density, 1);
        float round = width(Control.HOME, u, density, 1), click = width(Control.L3, u, density, 1);
        float rowStep = Math.max(.17f * u, round + 8 * density);
        float clickTop = height - .035f * u - click - .02f * u;
        float x, y;
        switch (control) {
            case LEFT_STICK:
                x = .28f * u;
                y = Math.min(height - .44f * u, clickTop - height(control, u, density, 1) / 2);
                break;
            case DPAD: x = .66f * u; y = height - .17f * u; break;
            case FACE:
                x = width - .27f * u;
                y = Math.min(height - .44f * u, clickTop - height(control, u, density, 1) / 2);
                break;
            case RIGHT_STICK: x = width - .68f * u; y = height - .19f * u; break;
            case LT: x = .045f * u + pillW / 2; y = .04f * u + pillH / 2; break;
            case LB: x = .045f * u + pillW / 2; y = .07f * u + pillH * 1.5f; break;
            case RT: x = width - .045f * u - pillW / 2; y = .04f * u + pillH / 2; break;
            case RB: x = width - .045f * u - pillW / 2; y = .07f * u + pillH * 1.5f; break;
            case L3: x = .035f * u + click / 2; y = height - .035f * u - click / 2; break;
            case R3: x = width - .035f * u - click / 2; y = height - .035f * u - click / 2; break;
            case SELECT: x = width / 2 - rowStep; y = .035f * u + round / 2; break;
            case START: x = width / 2 + rowStep; y = .035f * u + round / 2; break;
            default: x = width / 2; y = .035f * u + round / 2; break; // HOME
        }
        out[0] = x;
        out[1] = y;
    }

    /**
     * The control's box on a {@code width} x {@code height} surface as left, top, width and height in
     * pixels, written to {@code out} (four floats) and returned. The box always lies on the surface.
     */
    public float[] bounds(Control control, float width, float height, float density, float[] out) {
        Spec spec = spec(control);
        float u = unit(width, height, density);
        float w = width(control, u, density, spec.scale), h = height(control, u, density, spec.scale);
        // A control larger than the surface is shrunk to fit, keeping its shape.
        float fit = Math.min(1, Math.min(width / Math.max(1, w), height / Math.max(1, h)));
        w *= fit;
        h *= fit;
        float cx, cy;
        if (spec.placed()) {
            cx = spec.cx * width;
            cy = spec.cy * height;
        } else {
            defaultCenter(control, width, height, u, density, out);
            cx = out[0];
            cy = out[1];
        }
        out[0] = clamp(cx - w / 2, 0, Math.max(0, width - w));
        out[1] = clamp(cy - h / 2, 0, Math.max(0, height - h));
        out[2] = w;
        out[3] = h;
        return out;
    }

    /** "v1|ID,cx,cy,scale,opacity,hidden|..." for controls that differ from the default; "v1" alone for none. */
    public String serialize() {
        StringBuilder out = new StringBuilder(VERSION);
        for (Control control : Control.values()) {
            Spec spec = spec(control);
            if (spec.isDefault()) continue;
            out.append('|').append(control.name()).append(',')
                    .append(spec.placed() ? String.format(Locale.ROOT, "%.4f", spec.cx) : "-").append(',')
                    .append(spec.placed() ? String.format(Locale.ROOT, "%.4f", spec.cy) : "-").append(',')
                    .append(String.format(Locale.ROOT, "%.3f", spec.scale)).append(',')
                    .append(spec.opacity).append(',')
                    .append(spec.hidden ? '1' : '0');
        }
        return out.toString();
    }

    /**
     * Reads {@link #serialize} output. Unknown controls and broken entries are skipped and the rest
     * kept, so a layout from a newer version still loads what this one knows. Null or another version
     * gives the default layout.
     */
    public static PadLayout parse(String text) {
        PadLayout layout = new PadLayout();
        if (text == null) return layout;
        String[] parts = text.split("\\|");
        if (parts.length == 0 || !VERSION.equals(parts[0])) return layout;
        for (int i = 1; i < parts.length; i++) {
            String[] fields = parts[i].split(",", -1);
            if (fields.length < 6) continue;
            Control control = Control.named(fields[0]);
            if (control == null) continue;
            try {
                Spec spec = new Spec();
                if (!"-".equals(fields[1]) && !"-".equals(fields[2])) {
                    spec.cx = clamp(Float.parseFloat(fields[1]), 0, 1);
                    spec.cy = clamp(Float.parseFloat(fields[2]), 0, 1);
                    if (Float.isNaN(spec.cx) || Float.isNaN(spec.cy)) {
                        spec.cx = Float.NaN;
                        spec.cy = Float.NaN;
                    }
                }
                spec.scale = clamp(Float.parseFloat(fields[3]), MIN_SCALE, MAX_SCALE);
                if (Float.isNaN(spec.scale)) spec.scale = 1;
                spec.opacity = Math.max(MIN_OPACITY, Math.min(MAX_OPACITY, Integer.parseInt(fields[4])));
                spec.hidden = "1".equals(fields[5]);
                layout.spec(control).copyFrom(spec);
            } catch (NumberFormatException ignored) {
                // Skip this entry and keep the rest.
            }
        }
        return layout;
    }

    static float clamp(float value, float min, float max) {
        return Math.max(min, Math.min(max, value));
    }
}

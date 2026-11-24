package com.limelight.binding.input.pad;

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
        L3(Shape.ROUND), R3(Shape.ROUND), SELECT(Shape.ROUND), START(Shape.ROUND), HOME(Shape.ROUND),
        /** A PlayStation pad's touchpad click, below the top row as on the pad itself. */
        TOUCHPAD(Shape.PILL),
        /** A stick that holds W, A, S and D instead of moving an axis. Not in the default layout: Show adds it. */
        WASD(Shape.STICK, true),
        /**
         * A Steam Deck-like touchpad that moves the PC's pointer, or the camera as a right stick, with tap, right-click
         * and two-finger scroll. Not in the default layout: the editor adds it.
         */
        POINTER_PAD(Shape.TOUCHPAD, true),
        /**
         * The face cluster's four buttons as controls of their own, for a player who splits the cluster to place and
         * size each one: A bottom, B right, X left and Y top in the cluster, as {@link PadInput#FACE_CENTERS}. Left
         * out of every layout until the editor's Split shows them in the cluster's place, and never shown together
         * with the cluster ({@link #shown}); an older build skips their names and keeps the rest of the layout.
         */
        A(Shape.FACE_BUTTON, true), B(Shape.FACE_BUTTON, true), X(Shape.FACE_BUTTON, true), Y(Shape.FACE_BUTTON, true),
        /** The D-pad's four directions as controls of their own, split as the face buttons are. */
        DPAD_UP(Shape.DPAD_ARM, true), DPAD_DOWN(Shape.DPAD_ARM, true), DPAD_LEFT(Shape.DPAD_ARM, true),
        DPAD_RIGHT(Shape.DPAD_ARM, true),
        /**
         * A Steam Controller's (2026) trackpads, back buttons and grip touch, as the classic controller offers them.
         * They show only with the Steam Controller controls setting; {@link ModernController#forcedHidden} keeps them
         * off otherwise.
         */
        LEFT_PAD(Shape.PAD, false, true), RIGHT_PAD(Shape.PAD, false, true),
        L4(Shape.PILL, false, true), L5(Shape.PILL, false, true), R4(Shape.PILL, false, true), R5(Shape.PILL, false, true),
        LEFT_GRIP(Shape.PILL, false, true), RIGHT_GRIP(Shape.PILL, false, true);

        public final Shape shape;
        /** Whether a layout nobody edited leaves this control out. */
        public final boolean hiddenByDefault;
        /** Whether this is one of the Steam Controller controls, shown only with their setting. */
        public final boolean steam;

        Control(Shape shape) {
            this(shape, false, false);
        }

        Control(Shape shape, boolean hiddenByDefault) {
            this(shape, hiddenByDefault, false);
        }

        Control(Shape shape, boolean hiddenByDefault, boolean steam) {
            this.shape = shape;
            this.hiddenByDefault = hiddenByDefault;
            this.steam = steam;
        }

        static Control named(String name) {
            for (Control control : values()) {
                if (control.name().equals(name)) return control;
            }
            return null;
        }

        /** The cluster a split member belongs to: FACE for A, B, X and Y, DPAD for its directions; null otherwise. */
        public Control cluster() {
            switch (shape) {
                case FACE_BUTTON: return FACE;
                case DPAD_ARM: return DPAD;
                default: return null;
            }
        }

        /**
         * A member's place among its cluster's four: bottom, right, left and top for the face buttons, as
         * {@link PadInput#FACE_CENTERS}; up, down, left and right for the D-pad. -1 for every other control.
         */
        public int memberIndex() {
            switch (this) {
                case A: case DPAD_UP: return 0;
                case B: case DPAD_DOWN: return 1;
                case X: case DPAD_LEFT: return 2;
                case Y: case DPAD_RIGHT: return 3;
                default: return -1;
            }
        }
    }

    /** The four members a cluster splits into, in {@link Control#memberIndex} order; none for any other control. */
    public static Control[] members(Control cluster) {
        if (cluster == Control.FACE) return new Control[] {Control.A, Control.B, Control.X, Control.Y};
        if (cluster == Control.DPAD) {
            return new Control[] {Control.DPAD_UP, Control.DPAD_DOWN, Control.DPAD_LEFT, Control.DPAD_RIGHT};
        }
        return new Control[0];
    }

    /**
     * {@code PAD} is a Steam Controller trackpad: a rounded square. {@code TOUCHPAD} is the pointer touchpad, wider than
     * tall. {@code FACE_BUTTON} is one face button of a split cluster, {@code DPAD_ARM} one direction of a split D-pad.
     */
    public enum Shape { STICK, DPAD, FACE, PILL, ROUND, PAD, TOUCHPAD, FACE_BUTTON, DPAD_ARM }

    /** The face letters and shoulder names printed on the controls. */
    public enum Glyph { XBOX, PLAYSTATION, NINTENDO }

    /** What the pointer touchpad moves: the PC's pointer, or the camera as the right stick. */
    public enum PointerMode { MOUSE, CAMERA }

    /** A ready-made selection of controls; {@link #applyPreset} places them at their defaults. */
    public enum Preset { DEFAULT, MINIMAL, FULL }

    /**
     * The smallest and the largest scale a control's width or height stores. Whatever the scale, a control is never
     * smaller than a full touch ({@link #MIN_TOUCH_DP}, {@link #MIN_PILL_WIDTH_DP}) and never wider or taller than
     * the surface ({@link #bounds}); the editor grows a control up to the whole surface, and MAX_SCALE only bounds
     * what a broken or hand-written layout can hold. Layouts saved before free resizing stayed within 0.5 to 2.
     */
    public static final float MIN_SCALE = .5f, MAX_SCALE = 100f;
    public static final int MIN_OPACITY = 10, MAX_OPACITY = 100;
    /**
     * The longest side of the default unit: a tablet's controls stop growing past a large phone's. It sets the
     * default sizes only; a control resized in the editor can still fill the whole surface.
     */
    static final float MAX_UNIT_DP = 440;
    /** The smallest touch a control offers, and the narrowest a pill gets so it still reads as one. */
    static final float MIN_TOUCH_DP = 48, MIN_PILL_WIDTH_DP = 72;
    /** How far below the top row the stream's microphone and menu buttons reach: their 12 dp margin and 48 dp size. */
    static final float TOP_BUTTONS_DP = 12 + 48;

    /**
     * One control's settings. {@code cx} and {@code cy} are fractions of the surface; NaN means the default place.
     *
     * <p>{@code widthScale} and {@code heightScale} size the control's box independently, each a multiple of its
     * default width or height ({@link #baseWidth} or {@link #baseHeight} times the {@link #unit}), so a control can
     * be resized to any shape up to the whole surface. A layout saved before free resizing had one scale for both
     * and loads with the two equal, so it keeps its size and shape. {@link #serialize} writes the larger as the one
     * scale an older build reads and adds the two as the seventh and eighth fields.
     */
    public static final class Spec {
        public float cx = Float.NaN, cy = Float.NaN, widthScale = 1, heightScale = 1;
        public int opacity = MAX_OPACITY;
        public boolean hidden;
        /** The control's own default for {@link #hidden}; a spec that matches it is not saved. */
        final boolean defaultHidden;

        public Spec() {
            this(false);
        }

        Spec(boolean defaultHidden) {
            this.defaultHidden = defaultHidden;
            hidden = defaultHidden;
        }

        public boolean placed() {
            return !Float.isNaN(cx) && !Float.isNaN(cy);
        }

        /** The larger of the two scales: the size the editor shows, and the one scale an older build reads. */
        public float scale() {
            return Math.max(widthScale, heightScale);
        }

        void copyFrom(Spec other) {
            cx = other.cx; cy = other.cy; widthScale = other.widthScale; heightScale = other.heightScale;
            opacity = other.opacity; hidden = other.hidden;
        }

        boolean isDefault() {
            return !placed() && widthScale == 1 && heightScale == 1 && opacity == MAX_OPACITY && hidden == defaultHidden;
        }
    }

    private final Spec[] specs = new Spec[Control.values().length];
    private Glyph glyph = Glyph.XBOX;
    private PointerMode pointerMode = PointerMode.MOUSE;
    /**
     * The face cluster's buttons stand in the Nintendo arrangement ({@link PadInput#FACE_SWAPPED}): set by
     * {@link #join} from where the members stood, and kept by {@link #split}. Saved by {@link PadLayoutCodec}.
     */
    private boolean facesSwapped;

    public PadLayout() {
        for (Control control : Control.values()) specs[control.ordinal()] = new Spec(control.hiddenByDefault);
    }

    public Spec spec(Control control) {
        return specs[control.ordinal()];
    }

    public PadLayout copy() {
        PadLayout copy = new PadLayout();
        for (int i = 0; i < specs.length; i++) copy.specs[i].copyFrom(specs[i]);
        copy.glyph = glyph;
        copy.pointerMode = pointerMode;
        copy.facesSwapped = facesSwapped;
        return copy;
    }

    public void copyFrom(PadLayout other) {
        for (int i = 0; i < specs.length; i++) specs[i].copyFrom(other.specs[i]);
        glyph = other.glyph;
        pointerMode = other.pointerMode;
        facesSwapped = other.facesSwapped;
    }

    public boolean isDefault() {
        if (glyph != Glyph.XBOX || pointerMode != PointerMode.MOUSE || facesSwapped) return false;
        for (Spec spec : specs) {
            if (!spec.isDefault()) return false;
        }
        return true;
    }

    public void reset() {
        for (Spec spec : specs) spec.copyFrom(new Spec(spec.defaultHidden));
        glyph = Glyph.XBOX;
        pointerMode = PointerMode.MOUSE;
        facesSwapped = false;
    }

    /** Whether the face cluster's buttons stand in the Nintendo arrangement: B at the bottom, A right, Y left, X top. */
    public boolean facesSwapped() {
        return facesSwapped;
    }

    public void setFacesSwapped(boolean swapped) {
        facesSwapped = swapped;
    }

    public Glyph glyph() {
        return glyph;
    }

    public void setGlyph(Glyph glyph) {
        this.glyph = glyph == null ? Glyph.XBOX : glyph;
    }

    public PointerMode pointerMode() {
        return pointerMode;
    }

    public void setPointerMode(PointerMode mode) {
        pointerMode = mode == null ? PointerMode.MOUSE : mode;
    }

    /**
     * Shows the controls a preset names, at their default places, sizes and opacity: Minimal keeps the sticks, the
     * D-pad, the face cluster and View and Menu; Full adds every other gamepad control and the pointer touchpad. Both
     * keep the face cluster and the D-pad whole. The
     * Steam Controller controls follow their own setting and stay as they are; the WASD stick, a keyboard control,
     * stays opt-in like the pointer touchpad.
     */
    public void applyPreset(Preset preset) {
        // A preset chooses which controls show; the glyph family and what the pointer touchpad does are the
        // player's other choices and stay as they are, so the editor's buttons do not visibly revert.
        Glyph keptGlyph = glyph;
        PointerMode keptPointerMode = pointerMode;
        // Presets show the face cluster whole: in the arrangement its members stand in, split or all four hidden.
        if (spec(Control.FACE).hidden) takeFaceArrangement();
        boolean keptFacesSwapped = facesSwapped;
        Spec[] keptSteam = new Spec[specs.length];
        for (Control control : Control.values()) {
            if (!control.steam) continue;
            keptSteam[control.ordinal()] = new Spec(control.hiddenByDefault);
            keptSteam[control.ordinal()].copyFrom(spec(control));
        }
        reset();
        for (Control control : Control.values()) {
            if (control.steam) spec(control).copyFrom(keptSteam[control.ordinal()]);
        }
        glyph = keptGlyph;
        pointerMode = keptPointerMode;
        facesSwapped = keptFacesSwapped;
        if (preset == Preset.DEFAULT) return;
        for (Control control : Control.values()) {
            if (control.steam) continue;
            boolean shown;
            switch (preset) {
                case MINIMAL:
                    shown = control == Control.LEFT_STICK || control == Control.RIGHT_STICK || control == Control.DPAD ||
                            control == Control.FACE || control == Control.SELECT || control == Control.START;
                    break;
                case FULL:
                    // Presets keep the face cluster and the D-pad whole; a split is the player's own arrangement.
                    shown = control != Control.WASD && control.cluster() == null;
                    break;
                default:
                    shown = !control.hiddenByDefault;
                    break;
            }
            spec(control).hidden = !shown;
        }
    }

    /** Gives the control one scale for its width and height: its default shape at {@code scale} times its size. */
    public void setScale(Control control, float scale) {
        setSize(control, scale, scale);
    }

    /** Sizes the control's width and height independently, each as a multiple of its default ({@link Spec}). */
    public void setSize(Control control, float widthScale, float heightScale) {
        Spec spec = spec(control);
        spec.widthScale = clampScale(widthScale);
        spec.heightScale = clampScale(heightScale);
    }

    static float clampScale(float scale) {
        return Float.isNaN(scale) ? 1 : clamp(scale, MIN_SCALE, MAX_SCALE);
    }

    /**
     * Gives the control the box ({@code left}, {@code top}, {@code w}, {@code h}) in pixels of a {@code width} x
     * {@code height} surface: its center, and its width and height scales, each between a full touch and the
     * surface. The editor's corner handles size controls this way, and so can a layout carried over from pixel boxes.
     */
    public void setBounds(Control control, float left, float top, float w, float h, float width, float height,
                          float density) {
        float u = unit(width, height, density);
        setSize(control, Math.min(w, width) / Math.max(1e-6f, baseWidth(control) * u),
                Math.min(h, height) / Math.max(1e-6f, baseHeight(control) * u));
        moveTo(control, left + w / 2, top + h / 2, width, height, density);
    }

    /**
     * Scales the control from {@code widthScale} x {@code heightScale} by {@code factor} in both directions, keeping
     * its shape: the factor stops where a side would fall below {@link #MIN_SCALE} or grow past this surface.
     * Pinching and the editor's plus and minus buttons resize this way; the corner handles size each side alone.
     */
    public void scaleFrom(Control control, float widthScale, float heightScale, float factor, float width,
                          float height, float density) {
        float least = Math.max(MIN_SCALE / Math.max(1e-6f, widthScale), MIN_SCALE / Math.max(1e-6f, heightScale));
        float most = Float.MAX_VALUE;
        if (width > 0 && height > 0) {
            // As wide or as tall as the surface, whichever comes first; a surface not yet laid out sets no limit.
            float u = unit(width, height, density);
            most = Math.min(width / Math.max(1e-6f, baseWidth(control) * u * widthScale),
                    height / Math.max(1e-6f, baseHeight(control) * u * heightScale));
        }
        float f = Float.isNaN(factor) ? 1 : factor;
        // Where the two limits cross (a side at its smallest, another past a small surface), it can shrink, not grow.
        f = most >= least ? Math.max(least, Math.min(most, f)) : Math.min(f, 1);
        setSize(control, widthScale * f, heightScale * f);
    }

    /**
     * Whether the layout shows the control, the settings aside: not hidden, and for a split member, only while its
     * cluster is hidden, so a cluster and its members never show together.
     */
    public boolean shown(Control control) {
        if (spec(control).hidden) return false;
        Control cluster = control.cluster();
        return cluster == null || spec(cluster).hidden;
    }

    /** Whether a cluster is split: it is hidden and at least one of its members shows in its place. */
    public boolean isSplit(Control cluster) {
        if (!spec(cluster).hidden) return false;
        for (Control member : members(cluster)) {
            if (!spec(member).hidden) return true;
        }
        return false;
    }

    /** Splits the face cluster or the D-pad into its four members ({@link PadLayoutClusters#split}). */
    public void split(Control cluster, float width, float height, float density) {
        PadLayoutClusters.split(this, cluster, width, height, density);
    }

    /** Joins a split cluster again where its members stand ({@link PadLayoutClusters#join}). */
    public void join(Control cluster, float width, float height, float density) {
        PadLayoutClusters.join(this, cluster, width, height, density);
    }

    /** Takes the face cluster's arrangement from where its members stand ({@link PadLayoutClusters#takeFaceArrangement}). */
    public void takeFaceArrangement() {
        PadLayoutClusters.takeFaceArrangement(this);
    }

    static boolean arrangement(float[][] centers, boolean current) {
        return PadLayoutClusters.arrangement(centers, current);
    }

    static void memberBox(Control member, float left, float top, float width, float height, float density, float[] out) {
        PadLayoutClusters.memberBox(member, left, top, width, height, density, out);
    }

    static float[][] memberBoxes(Control cluster, float left, float top, float width, float height, float surfaceWidth,
                                 float surfaceHeight, float density) {
        return PadLayoutClusters.memberBoxes(cluster, left, top, width, height, surfaceWidth, surfaceHeight, density);
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

    static float unit(float width, float height, float density) {
        return PadLayoutDefaults.unit(width, height, density);
    }

    static float baseWidth(Control control) {
        return PadLayoutDefaults.baseWidth(control);
    }

    static float baseHeight(Control control) {
        return PadLayoutDefaults.baseHeight(control);
    }

    static float width(Control control, float u, float density, float scale) {
        return PadLayoutDefaults.width(control, u, density, scale);
    }

    static float height(Control control, float u, float density, float scale) {
        return PadLayoutDefaults.height(control, u, density, scale);
    }

    static void defaultCenter(Control control, float width, float height, float u, float density, float[] out) {
        PadLayoutDefaults.defaultCenter(control, width, height, u, density, out);
    }

    static float[] steamCenters(float width, float height, float u, float density) {
        return PadLayoutDefaults.steamCenters(width, height, u, density);
    }

    /**
     * The control's box on a {@code width} x {@code height} surface as left, top, width and height in
     * pixels, written to {@code out} (four floats) and returned. The box always lies on the surface.
     */
    public float[] bounds(Control control, float width, float height, float density, float[] out) {
        Spec spec = spec(control);
        float u = unit(width, height, density);
        // Never wider or taller than the surface: a control grown to the whole surface fills it, and one sized on a
        // larger surface (before a fold or a window resize) fits again. A scale of 2 or less, all a layout saved
        // before free resizing has, never reaches the surface's size, so such a layout keeps its shape.
        float w = Math.min(width(control, u, density, spec.widthScale), Math.max(0, width));
        float h = Math.min(height(control, u, density, spec.heightScale), Math.max(0, height));
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

    /** Serializes edits to the v1 wire format ({@link PadLayoutCodec#serialize}). */
    public String serialize() {
        return PadLayoutCodec.serialize(this);
    }

    /** Reads serialized v1/v2 layouts ({@link PadLayoutCodec#parse}). */
    public static PadLayout parse(String text) {
        return PadLayoutCodec.parse(text);
    }

    static float clamp(float value, float min, float max) {
        return Math.max(min, Math.min(max, value));
    }
}

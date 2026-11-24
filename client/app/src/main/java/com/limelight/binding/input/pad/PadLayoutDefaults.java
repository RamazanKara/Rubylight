package com.limelight.binding.input.pad;

/**
 * Default control sizing, screen-adaptive centers, and Steam Controller fallback arrangements for
 * {@link PadLayout}.
 */
final class PadLayoutDefaults {
    /** The D-pad's and the face cluster's side at scale 1, in units. */
    private static final float DPAD_BASE = .30f, FACE_BASE = .40f;
    private static final int STEAM_FIRST = PadLayout.Control.LEFT_PAD.ordinal();
    private static final int STEAM_COUNT = PadLayout.Control.values().length - STEAM_FIRST;

    private PadLayoutDefaults() {
    }

    /**
     * The default size unit: the short side of the surface, capped for tablets, and narrow enough
     * that the left and right clusters (about 1.75 units together) never meet on a portrait phone.
     */
    static float unit(float width, float height, float density) {
        return Math.min(Math.min(Math.min(width, height), PadLayout.MAX_UNIT_DP * density), width / 1.75f);
    }

    /** Width and height of a control at scale 1, in units. */
    static float baseWidth(PadLayout.Control control) {
        switch (control.shape) {
            // The WASD stick takes the right stick's size so its default place fits between the clusters.
            case STICK: return control == PadLayout.Control.LEFT_STICK ? .42f : .34f;
            case DPAD: return DPAD_BASE;
            case FACE: return FACE_BASE;
            case PILL: return .25f;
            case PAD: return .22f;
            case TOUCHPAD: return .30f;
            // A split member's box at scale 1 is its button in the default-sized cluster (memberBox).
            case FACE_BUTTON: return FACE_BASE * 2 * PadInput.FACE_RADIUS;
            case DPAD_ARM: return control == PadLayout.Control.DPAD_UP || control == PadLayout.Control.DPAD_DOWN ?
                    DPAD_BASE * PadInput.DPAD_ARM :
                    DPAD_BASE * ((1 - PadInput.DPAD_ARM) / 2 + PadInput.DPAD_CORNER);
            default: return control == PadLayout.Control.L3 || control == PadLayout.Control.R3 ? .13f : .11f;
        }
    }

    static float baseHeight(PadLayout.Control control) {
        if (control.shape == PadLayout.Shape.PILL) return .12f;
        if (control.shape == PadLayout.Shape.TOUCHPAD) return .19f;
        if (control.shape == PadLayout.Shape.DPAD_ARM) {
            return control == PadLayout.Control.DPAD_UP || control == PadLayout.Control.DPAD_DOWN ?
                    DPAD_BASE * ((1 - PadInput.DPAD_ARM) / 2 + PadInput.DPAD_CORNER) : DPAD_BASE * PadInput.DPAD_ARM;
        }
        return baseWidth(control);
    }

    /**
     * Width and height of a control in pixels at {@code scale}: never below a 48 dp touch, and the
     * pill shoulders and triggers never narrower than 72 dp, whatever the surface or the scale.
     */
    static float width(PadLayout.Control control, float u, float density, float scale) {
        return Math.max(baseWidth(control) * u * scale,
                (control.shape == PadLayout.Shape.PILL ? PadLayout.MIN_PILL_WIDTH_DP : PadLayout.MIN_TOUCH_DP) * density);
    }

    static float height(PadLayout.Control control, float u, float density, float scale) {
        return Math.max(baseHeight(control) * u * scale, PadLayout.MIN_TOUCH_DP * density);
    }

    /**
     * The default center of a control, in pixels, from the nearest edges of the surface. Edge controls
     * keep their margins from their own size, so a control grown to its minimum touch on a small
     * surface still clears its neighbours: the shoulder pair, the top row and the stick clicks below
     * the clusters.
     */
    static void defaultCenter(PadLayout.Control control, float width, float height, float u, float density, float[] out) {
        float pillW = width(PadLayout.Control.LT, u, density, 1), pillH = height(PadLayout.Control.LT, u, density, 1);
        float round = width(PadLayout.Control.HOME, u, density, 1), click = width(PadLayout.Control.L3, u, density, 1);
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
            case WASD: {
                // Between the clusters and below the touchpad, clear of every default control until it is moved, and
                // of the microphone and menu buttons, which sit below the top row. Where the clusters leave no room
                // below the buttons (16:10 and short 16:9 screens), it keeps its spot right below the touchpad;
                // the buttons can be dragged.
                float touchpadBottom = .035f * u + round + Math.max(.02f * u, 8 * density) + pillH;
                float buttonsBottom = touchpadBottom + PadLayout.TOP_BUTTONS_DP * density;
                float size = height(control, u, density, 1);
                x = width / 2;
                y = Math.max(height / 2, buttonsBottom + 8 * density + size / 2);
                if (!clearOfDefaults(control, x, y, size, width, height, u, density)) {
                    y = Math.max(height / 2, touchpadBottom + 8 * density + size / 2);
                }
                break;
            }
            case TOUCHPAD: x = width / 2; y = .035f * u + round + Math.max(.02f * u, 8 * density) + pillH / 2; break;
            case HOME: x = width / 2; y = .035f * u + round / 2; break;
            case POINTER_PAD: {
                // The middle of the surface, where a phone held in two hands leaves the thumb room, but never under the
                // stream's microphone and menu buttons, which sit below the top row: on a short surface (a phone in
                // landscape) it moves down below them, as far as it stays on the surface. Both it and the WASD stick
                // are left out of the default layout; with both added, one gets moved.
                float touchpadBottom = .035f * u + round + Math.max(.02f * u, 8 * density) + pillH;
                float buttonsBottom = touchpadBottom + PadLayout.TOP_BUTTONS_DP * density;
                float size = height(control, u, density, 1);
                x = width / 2;
                y = Math.min(Math.max(height / 2, buttonsBottom + 8 * density + size / 2), height - size / 2);
                break;
            }
            case A: case B: case X: case Y:
            case DPAD_UP: case DPAD_DOWN: case DPAD_LEFT: case DPAD_RIGHT: {
                // A split member never placed (the editor's Split always places them) sits where Split puts it from
                // the default cluster.
                PadLayout.Control cluster = control.cluster();
                defaultCenter(cluster, width, height, u, density, out);
                float w = width(cluster, u, density, 1), h = height(cluster, u, density, 1);
                float[][] boxes = PadLayoutClusters.memberBoxes(cluster, out[0] - w / 2, out[1] - h / 2, w, h, width,
                        height, density);
                PadLayout.Control[] members = PadLayout.members(cluster);
                float[] member = boxes[0];
                for (int i = 0; i < members.length; i++) {
                    if (members[i] == control) member = boxes[i];
                }
                x = member[0] + member[2] / 2;
                y = member[1] + member[3] / 2;
                break;
            }
            default: {
                // A Steam Controller control: its place comes from the arrangement the surface has room for.
                float[] centers = steamCenters(width, height, u, density);
                int i = control.ordinal() - STEAM_FIRST;
                x = centers[2 * i];
                y = centers[2 * i + 1];
                break;
            }
        }
        out[0] = x;
        out[1] = y;
    }

    /** Whether a square {@code size} control centred at x, y stays off every other control at its default place. */
    private static boolean clearOfDefaults(PadLayout.Control self, float x, float y, float size, float width,
                                           float height, float u, float density) {
        if (y + size / 2 > height) return false;
        float[] center = new float[2];
        for (PadLayout.Control other : PadLayout.Control.values()) {
            // The controls the default layout leaves out, and the Steam Controller controls a setting keeps off, do not
            // push the WASD stick from its place; when both optional middle controls are shown, one gets moved.
            if (other == self || other.steam || other.hiddenByDefault) continue;
            defaultCenter(other, width, height, u, density, center);
            float w = width(other, u, density, 1), h = height(other, u, density, 1);
            if (Math.abs(center[0] - x) < (w + size) / 2 && Math.abs(center[1] - y) < (h + size) / 2) return false;
        }
        return true;
    }

    /**
     * The default centers of the Steam Controller controls, x and y for each in {@link PadLayout.Control} order. Three
     * arrangements are tried, each first below the stream's microphone and menu buttons and then right below the top
     * row, and the first that keeps every one on the surface and off every default control and each other wins:
     * <ol>
     * <li>wide surfaces: the back buttons inward of the shoulders and triggers, the trackpads side by side in the
     * middle with a grip outside each;</li>
     * <li>narrower ones: the same, with the two grips stacked between the trackpads;</li>
     * <li>tall ones (portrait): rows of L4 and R4, L5 and R5 and the grips in the middle, the trackpads below.</li>
     * </ol>
     * Where none fits (the smallest phones), the first stands; every control can be moved, resized or hidden.
     */
    static float[] steamCenters(float width, float height, float u, float density) {
        float[] centers = new float[STEAM_COUNT * 2];
        for (boolean belowButtons : new boolean[] {true, false}) {
            for (int arrangement = 0; arrangement < 3; arrangement++) {
                steamArrangement(arrangement, belowButtons, width, u, density, centers);
                if (steamClear(centers, width, height, u, density)) return centers;
            }
        }
        steamArrangement(0, false, width, u, density, centers);
        return centers;
    }

    private static void steamArrangement(int arrangement, boolean belowButtons, float width, float u, float density,
                                         float[] out) {
        float pillW = width(PadLayout.Control.L4, u, density, 1), pillH = height(PadLayout.Control.L4, u, density, 1);
        float pad = width(PadLayout.Control.LEFT_PAD, u, density, 1), round = width(PadLayout.Control.HOME, u, density, 1);
        float gap = Math.max(.02f * u, 8 * density);
        // Below the touchpad's default place, and below the microphone and menu buttons that sit under it.
        float top = .035f * u + round + gap + pillH + (belowButtons ? PadLayout.TOP_BUTTONS_DP * density : 0);
        float backX = .045f * u + pillW + gap + pillW / 2, triggerY = .04f * u + pillH / 2, shoulderY = .07f * u + pillH * 1.5f;
        if (arrangement < 2) {
            steamAt(out, PadLayout.Control.L4, backX, triggerY);
            steamAt(out, PadLayout.Control.L5, backX, shoulderY);
            steamAt(out, PadLayout.Control.R4, width - backX, triggerY);
            steamAt(out, PadLayout.Control.R5, width - backX, shoulderY);
        }
        if (arrangement == 0) {
            float cy = top + gap + pad / 2, gripX = width / 2 - pad - gap / 2 - gap - pillW / 2;
            steamAt(out, PadLayout.Control.LEFT_PAD, width / 2 - pad / 2 - gap / 2, cy);
            steamAt(out, PadLayout.Control.RIGHT_PAD, width / 2 + pad / 2 + gap / 2, cy);
            steamAt(out, PadLayout.Control.LEFT_GRIP, gripX, cy);
            steamAt(out, PadLayout.Control.RIGHT_GRIP, width - gripX, cy);
        } else if (arrangement == 1) {
            float cy = top + gap + Math.max(pad, 2 * pillH + gap) / 2, offset = pillW / 2 + gap + pad / 2;
            steamAt(out, PadLayout.Control.LEFT_PAD, width / 2 - offset, cy);
            steamAt(out, PadLayout.Control.RIGHT_PAD, width / 2 + offset, cy);
            steamAt(out, PadLayout.Control.LEFT_GRIP, width / 2, cy - gap / 2 - pillH / 2);
            steamAt(out, PadLayout.Control.RIGHT_GRIP, width / 2, cy + gap / 2 + pillH / 2);
        } else {
            float left = width / 2 - gap / 2 - pillW / 2, right = width / 2 + gap / 2 + pillW / 2;
            float y = top + gap + pillH / 2;
            steamAt(out, PadLayout.Control.L4, left, y);
            steamAt(out, PadLayout.Control.R4, right, y);
            y += pillH + gap;
            steamAt(out, PadLayout.Control.L5, left, y);
            steamAt(out, PadLayout.Control.R5, right, y);
            y += pillH + gap;
            steamAt(out, PadLayout.Control.LEFT_GRIP, left, y);
            steamAt(out, PadLayout.Control.RIGHT_GRIP, right, y);
            y += pillH / 2 + gap + pad / 2;
            steamAt(out, PadLayout.Control.LEFT_PAD, width / 2 - pad / 2 - gap / 2, y);
            steamAt(out, PadLayout.Control.RIGHT_PAD, width / 2 + pad / 2 + gap / 2, y);
        }
    }

    private static void steamAt(float[] out, PadLayout.Control control, float x, float y) {
        int i = control.ordinal() - STEAM_FIRST;
        out[2 * i] = x;
        out[2 * i + 1] = y;
    }

    /** Whether the Steam Controller controls at these centers lie on the surface, apart from each other and every default control. */
    private static boolean steamClear(float[] centers, float width, float height, float u, float density) {
        PadLayout.Control[] controls = PadLayout.Control.values();
        float[] center = new float[2];
        for (int i = 0; i < STEAM_COUNT; i++) {
            PadLayout.Control control = controls[STEAM_FIRST + i];
            float w = width(control, u, density, 1), h = height(control, u, density, 1);
            float x = centers[2 * i], y = centers[2 * i + 1];
            if (x - w / 2 < 0 || y - h / 2 < 0 || x + w / 2 > width || y + h / 2 > height) return false;
            for (PadLayout.Control other : controls) {
                if (other.hiddenByDefault) continue;
                float ow = width(other, u, density, 1), oh = height(other, u, density, 1);
                if (other.steam) {
                    int j = other.ordinal() - STEAM_FIRST;
                    if (j <= i) continue;
                    center[0] = centers[2 * j];
                    center[1] = centers[2 * j + 1];
                } else {
                    defaultCenter(other, width, height, u, density, center);
                }
                if (Math.abs(center[0] - x) < (w + ow) / 2 && Math.abs(center[1] - y) < (h + oh) / 2) return false;
            }
        }
        return true;
    }
}

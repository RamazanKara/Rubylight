package com.limelight.binding.input.pad;

import com.limelight.binding.input.LayoutStick;
import com.limelight.nvstream.input.ControllerPacket;

/** Where a finger on a modern control lands, as gamepad input. Plain math, no allocation. */
public final class PadInput {
    /** Face button centers as fractions of the cluster: A bottom, B right, X left, Y top, as on an Xbox pad. */
    static final float[][] FACE_CENTERS = {{.5f, .78f}, {.78f, .5f}, {.22f, .5f}, {.5f, .22f}};
    static final int[] FACE_FLAGS = {ControllerPacket.A_FLAG, ControllerPacket.B_FLAG,
            ControllerPacket.X_FLAG, ControllerPacket.Y_FLAG};
    static final String[] FACE_LABELS = {"A", "B", "X", "Y"};
    /** A face button's radius, how far from its center a finger still presses it, and the chord margin. */
    static final float FACE_RADIUS = .19f, FACE_REACH = .30f, FACE_CHORD = .07f, FACE_CENTER_GAP = .10f;
    /** A second press on a stick within this time after letting go holds the stick's click (L3 or R3). */
    public static final long STICK_CLICK_MS = 300, STICK_TAP_MS = 200;
    static final float STICK_TAP_PUSH = .5f;

    private PadInput() { }

    /**
     * The face buttons a finger at ({@code x}, {@code y}) presses in a cluster of {@code size} pixels.
     * A finger between two neighbours presses both, so A+B is one thumb; the very middle presses none.
     */
    public static int face(float x, float y, float size) {
        if (size <= 0) return 0;
        float fx = x / size, fy = y / size;
        float mx = fx - .5f, my = fy - .5f;
        if (mx * mx + my * my < FACE_CENTER_GAP * FACE_CENTER_GAP) return 0;
        int nearest = -1, second = -1;
        float nearestD = Float.MAX_VALUE, secondD = Float.MAX_VALUE;
        for (int i = 0; i < FACE_CENTERS.length; i++) {
            float dx = fx - FACE_CENTERS[i][0], dy = fy - FACE_CENTERS[i][1];
            float d = (float) Math.sqrt(dx * dx + dy * dy);
            if (d < nearestD) {
                second = nearest; secondD = nearestD;
                nearest = i; nearestD = d;
            } else if (d < secondD) {
                second = i; secondD = d;
            }
        }
        if (nearestD > FACE_REACH) return 0;
        int flags = FACE_FLAGS[nearest];
        if (second >= 0 && secondD <= FACE_REACH && secondD - nearestD < FACE_CHORD) flags |= FACE_FLAGS[second];
        return flags;
    }

    /** The D-pad directions a finger at ({@code x}, {@code y}) holds on a pad of {@code size} pixels, as gamepad flags. */
    public static int dpad(float x, float y, float size) {
        if (size <= 0) return 0;
        float half = size / 2;
        float nx = (x - half) / half, ny = (half - y) / half;
        if (nx * nx + ny * ny < LayoutStick.DPAD_DEADZONE * LayoutStick.DPAD_DEADZONE) return 0;
        return LayoutStick.dpadFlags(LayoutStick.directions(nx, ny, LayoutStick.DPAD_DIAGONAL));
    }

    /** Whether a stick touch was a tap: short and barely pushed. Only a tap arms the stick's click. */
    public static boolean stickTap(long heldMs, float push) {
        return heldMs >= 0 && heldMs <= STICK_TAP_MS && push < STICK_TAP_PUSH;
    }

    /** Whether a stick touched at {@code downAt} after letting go at {@code lastUpAt} holds its click. */
    public static boolean stickClick(long lastUpAt, long downAt) {
        return lastUpAt > 0 && downAt >= lastUpAt && downAt - lastUpAt <= STICK_CLICK_MS;
    }

    /** The button a simple control holds, as a gamepad flag; 0 for the sticks, the D-pad, the face cluster and the triggers. */
    public static int buttonFlag(PadLayout.Control control) {
        switch (control) {
            case LB: return ControllerPacket.LB_FLAG;
            case RB: return ControllerPacket.RB_FLAG;
            case L3: case LEFT_STICK: return ControllerPacket.LS_CLK_FLAG;
            case R3: case RIGHT_STICK: return ControllerPacket.RS_CLK_FLAG;
            case SELECT: return ControllerPacket.BACK_FLAG;
            case START: return ControllerPacket.PLAY_FLAG;
            case HOME: return ControllerPacket.SPECIAL_BUTTON_FLAG;
            default: return 0;
        }
    }

    public static boolean isTrigger(PadLayout.Control control) {
        return control == PadLayout.Control.LT || control == PadLayout.Control.RT;
    }
}

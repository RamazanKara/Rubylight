package com.limelight.binding.input.virtual_controller;

import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.nvstream.jni.MoonBridge;

/**
 * The on-screen controls a Steam Controller (2026) has and a phone's pad lacks: two trackpads,
 * the back buttons L4, R4, L5 and R5, and grip touch, for a PC whose controller type is the Steam
 * Controller. Flags and capability bits match {@code client/protocol/core/src/steam_controller.rs}.
 *
 * <p>The trackpads ride on touchpad 0, which such a PC splits into the left pad (x below 0.5) and
 * the right pad, each half stretched back to the pad's full width.
 */
public final class SteamControls {
    private SteamControls() {}

    /** Arrival capability: grip touch through the grip touch flags. */
    public static final short LI_CCAP_GRIP_SENSE = 0x200;
    /** Arrival capability: stick touch through the stick touch flags. */
    public static final short LI_CCAP_STICK_TOUCH = 0x400;

    /** Held, not pressed: in the upper half of the button word. */
    public static final int LEFT_GRIP_TOUCH_FLAG = 0x0040_0000;
    public static final int RIGHT_GRIP_TOUCH_FLAG = 0x0080_0000;
    public static final int LEFT_STICK_TOUCH_FLAG = 0x0100_0000;
    public static final int RIGHT_STICK_TOUCH_FLAG = 0x0200_0000;

    /** The back buttons as the PC maps them: R4, L4, R5, L5 are paddles 1 to 4. */
    public static final int R4_FLAG = ControllerPacket.PADDLE1_FLAG;
    public static final int L4_FLAG = ControllerPacket.PADDLE2_FLAG;
    public static final int R5_FLAG = ControllerPacket.PADDLE3_FLAG;
    public static final int L5_FLAG = ControllerPacket.PADDLE4_FLAG;

    /** The buttons the controls add to their pad's arrival. */
    public static final int BUTTONS = R4_FLAG | L4_FLAG | R5_FLAG | L5_FLAG;

    /**
     * The capabilities the controls add to their pad's arrival. Not the independent pad and
     * trigger clicks (SC26 buttons): those would make every click explicit, which these controls
     * cannot send.
     */
    public static final short CAPABILITIES =
            (short) (MoonBridge.LI_CCAP_TOUCHPAD | LI_CCAP_GRIP_SENSE | LI_CCAP_STICK_TOUCH);

    public static final int LEFT_PAD = 0, RIGHT_PAD = 1;

    /** Finger ids of the two pads, apart from the small ids a physical pad's touchpad uses. */
    private static final int POINTER_BASE = 0x100;

    /** The largest x on the left pad: just short of the right pad's half. */
    private static final float LEFT_PAD_MAX_X = 0.4999f;

    /** The finger id a pad's touches carry. */
    public static int pointerId(int pad) {
        return POINTER_BASE + pad;
    }

    /** Touchpad 0's x for a point across a pad, 0 at its left edge and 1 at its right. */
    public static float touchX(int pad, float across) {
        float half = clamp(across) * 0.5f;
        return pad == LEFT_PAD ? Math.min(half, LEFT_PAD_MAX_X) : 0.5f + half;
    }

    /** Touchpad 0's y for a point down a pad, 0 at its top and 1 at its bottom. */
    public static float touchY(float down) {
        return clamp(down);
    }

    /** A screen's touch pressure as the touchpad's: within 0 to 1. */
    public static float pressure(float pressure) {
        return clamp(pressure);
    }

    private static float clamp(float value) {
        if (Float.isNaN(value)) return 0f;
        return Math.max(0f, Math.min(1f, value));
    }
}

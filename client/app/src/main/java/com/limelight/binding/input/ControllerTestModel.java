package com.limelight.binding.input;

import android.view.KeyEvent;
import android.view.MotionEvent;

import com.limelight.nvstream.input.ControllerPacket;

import java.util.Set;

/** What the controller test page shows, worked out the way a stream reads the same controller. */
public final class ControllerTestModel {
    // Triggers are analog in the protocol; the page also shows them as pressed past half travel.
    public static final int LT_FLAG = 1 << 28;
    public static final int RT_FLAG = 1 << 29;

    /** The order of the button chips on the page. */
    public static final int[] BUTTONS = {
            ControllerPacket.A_FLAG, ControllerPacket.B_FLAG, ControllerPacket.X_FLAG, ControllerPacket.Y_FLAG,
            ControllerPacket.LB_FLAG, ControllerPacket.RB_FLAG, LT_FLAG, RT_FLAG,
            ControllerPacket.LS_CLK_FLAG, ControllerPacket.RS_CLK_FLAG,
            ControllerPacket.BACK_FLAG, ControllerPacket.PLAY_FLAG, ControllerPacket.SPECIAL_BUTTON_FLAG,
            ControllerPacket.UP_FLAG, ControllerPacket.DOWN_FLAG, ControllerPacket.LEFT_FLAG, ControllerPacket.RIGHT_FLAG,
            ControllerPacket.TOUCHPAD_FLAG, ControllerPacket.MISC_FLAG,
            ControllerPacket.PADDLE1_FLAG, ControllerPacket.PADDLE2_FLAG,
            ControllerPacket.PADDLE3_FLAG, ControllerPacket.PADDLE4_FLAG,
    };

    public enum Motor { WEAK, STRONG, LEFT_TRIGGER, RIGHT_TRIGGER }

    private ControllerTestModel() {}

    /** The button an Android controller key lights up, or 0 for keys the page does not show. */
    public static int buttonFlag(int keyCode) {
        switch (keyCode) {
            case KeyEvent.KEYCODE_BUTTON_A: return ControllerPacket.A_FLAG;
            case KeyEvent.KEYCODE_BUTTON_B: return ControllerPacket.B_FLAG;
            case KeyEvent.KEYCODE_BUTTON_X: return ControllerPacket.X_FLAG;
            case KeyEvent.KEYCODE_BUTTON_Y: return ControllerPacket.Y_FLAG;
            case KeyEvent.KEYCODE_BUTTON_L1: return ControllerPacket.LB_FLAG;
            case KeyEvent.KEYCODE_BUTTON_R1: return ControllerPacket.RB_FLAG;
            case KeyEvent.KEYCODE_BUTTON_L2: return LT_FLAG;
            case KeyEvent.KEYCODE_BUTTON_R2: return RT_FLAG;
            case KeyEvent.KEYCODE_BUTTON_THUMBL: return ControllerPacket.LS_CLK_FLAG;
            case KeyEvent.KEYCODE_BUTTON_THUMBR: return ControllerPacket.RS_CLK_FLAG;
            case KeyEvent.KEYCODE_BACK:
            case KeyEvent.KEYCODE_BUTTON_SELECT: return ControllerPacket.BACK_FLAG;
            case KeyEvent.KEYCODE_MENU:
            case KeyEvent.KEYCODE_BUTTON_START: return ControllerPacket.PLAY_FLAG;
            case KeyEvent.KEYCODE_BUTTON_MODE: return ControllerPacket.SPECIAL_BUTTON_FLAG;
            case KeyEvent.KEYCODE_DPAD_UP: return ControllerPacket.UP_FLAG;
            case KeyEvent.KEYCODE_DPAD_DOWN: return ControllerPacket.DOWN_FLAG;
            case KeyEvent.KEYCODE_DPAD_LEFT: return ControllerPacket.LEFT_FLAG;
            case KeyEvent.KEYCODE_DPAD_RIGHT: return ControllerPacket.RIGHT_FLAG;
            case KeyEvent.KEYCODE_MEDIA_RECORD: return ControllerPacket.MISC_FLAG;
            default: return 0;
        }
    }

    /** D-pad directions from hat axes, using the stream's half-travel threshold. */
    public static int hatFlags(float hatX, float hatY) {
        int flags = 0;
        if (hatX < -0.5f) flags |= ControllerPacket.LEFT_FLAG;
        else if (hatX > 0.5f) flags |= ControllerPacket.RIGHT_FLAG;
        if (hatY < -0.5f) flags |= ControllerPacket.UP_FLAG;
        else if (hatY > 0.5f) flags |= ControllerPacket.DOWN_FLAG;
        return flags;
    }

    /** Digital LT and RT for the chips, from analog travel between 0 and 1. */
    public static int triggerFlags(float leftTrigger, float rightTrigger) {
        return (leftTrigger > 0.5f ? LT_FLAG : 0) | (rightTrigger > 0.5f ? RT_FLAG : 0);
    }

    /**
     * The separately driven motors behind a controller's vibrator IDs, in ID order. Two vibrators
     * are the weak and strong motors and four add the triggers, as during a stream; any other
     * count only vibrates as a whole.
     */
    public static Motor[] motors(int vibratorCount) {
        switch (vibratorCount) {
            case 2: return new Motor[] {Motor.WEAK, Motor.STRONG};
            case 4: return new Motor[] {Motor.WEAK, Motor.STRONG, Motor.LEFT_TRIGGER, Motor.RIGHT_TRIGGER};
            default: return new Motor[0];
        }
    }

    /** Android 12 can crash in its controller sensor service, so only Sony and Nintendo pads are probed there. */
    public static boolean canProbeSensors(int sdk, int vendorId) {
        return sdk >= 33 || (sdk >= 31 && (vendorId == 0x054c || vendorId == 0x057e));
    }

    /** Before Android 14 every light claims RGB control, so only Sony pads are taken at their word. */
    public static boolean trustRgbLight(int sdk, int vendorId, boolean hasRgbControl) {
        return sdk >= 31 && hasRgbControl && (sdk >= 34 || vendorId == 0x054c);
    }

    /** Which motion axes carry the sticks and triggers, chosen as the stream chooses them. */
    public static final class Layout {
        public int rightStickX = -1, rightStickY = -1, leftTrigger = -1, rightTrigger = -1;
        public boolean triggersIdleNegative;

        public float trigger(float raw) {
            float value = triggersIdleNegative ? (raw + 1) / 2 : raw;
            return Math.max(0, Math.min(1, value));
        }
    }

    public static Layout layout(Set<Integer> axes, boolean nonStandardDualShock4) {
        Layout layout = new Layout();
        boolean rxRy = axes.contains(MotionEvent.AXIS_RX) && axes.contains(MotionEvent.AXIS_RY);
        boolean zRz = axes.contains(MotionEvent.AXIS_Z) && axes.contains(MotionEvent.AXIS_RZ);
        if (axes.contains(MotionEvent.AXIS_LTRIGGER) && axes.contains(MotionEvent.AXIS_RTRIGGER)) {
            layout.leftTrigger = MotionEvent.AXIS_LTRIGGER;
            layout.rightTrigger = MotionEvent.AXIS_RTRIGGER;
        }
        else if (axes.contains(MotionEvent.AXIS_BRAKE) && axes.contains(MotionEvent.AXIS_GAS)) {
            layout.leftTrigger = MotionEvent.AXIS_BRAKE;
            layout.rightTrigger = MotionEvent.AXIS_GAS;
        }
        else if (axes.contains(MotionEvent.AXIS_BRAKE) && axes.contains(MotionEvent.AXIS_THROTTLE)) {
            layout.leftTrigger = MotionEvent.AXIS_BRAKE;
            layout.rightTrigger = MotionEvent.AXIS_THROTTLE;
        }
        else if (rxRy) {
            if (nonStandardDualShock4) {
                layout.leftTrigger = MotionEvent.AXIS_RX;
                layout.rightTrigger = MotionEvent.AXIS_RY;
            }
            else {
                layout.rightStickX = MotionEvent.AXIS_RX;
                layout.rightStickY = MotionEvent.AXIS_RY;
                if (zRz) {
                    layout.leftTrigger = MotionEvent.AXIS_Z;
                    layout.rightTrigger = MotionEvent.AXIS_RZ;
                }
            }
            layout.triggersIdleNegative = true;
        }
        if (layout.rightStickX == -1) {
            if (zRz && layout.leftTrigger != MotionEvent.AXIS_Z) {
                layout.rightStickX = MotionEvent.AXIS_Z;
                layout.rightStickY = MotionEvent.AXIS_RZ;
            }
            else if (rxRy && layout.leftTrigger != MotionEvent.AXIS_RX) {
                layout.rightStickX = MotionEvent.AXIS_RX;
                layout.rightStickY = MotionEvent.AXIS_RY;
            }
        }
        return layout;
    }
}

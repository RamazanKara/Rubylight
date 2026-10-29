package com.limelight.ui;

import android.annotation.SuppressLint;
import android.os.Build;
import android.view.HapticFeedbackConstants;
import android.view.View;

/**
 * Touch feedback for the app's own controls: the PC keyboard's keys, slider steps, settings switches and the shell
 * bar. View.performHapticFeedback follows the system's touch feedback setting, so there is no switch of our own.
 * Each kind takes the newest constant the device knows and the closest older one below it.
 */
public final class Haptics {
    public enum Kind {
        /** A key of an on-screen keyboard going down. */
        KEY,
        /** A button that moves to another screen, such as PCs or Settings in the shell bar. */
        NAVIGATION,
        /** A slider's value moving one step. */
        STEP,
        /** A switch turned on. */
        TOGGLE_ON,
        /** A switch turned off. */
        TOGGLE_OFF
    }

    private Haptics() {
    }

    /** The feedback constant for {@code kind} on a device running API level {@code sdk}. */
    @SuppressLint("InlinedApi") // Each newer constant is used only from its API level on.
    static int constant(Kind kind, int sdk) {
        switch (kind) {
            case KEY:
                return HapticFeedbackConstants.KEYBOARD_TAP;
            case STEP:
                // A slider steps many times in one drag: the light tick Android 14 made for that, a clock tick before.
                return sdk >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE ? HapticFeedbackConstants.SEGMENT_FREQUENT_TICK
                        : HapticFeedbackConstants.CLOCK_TICK;
            case TOGGLE_ON:
                return sdk >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE ? HapticFeedbackConstants.TOGGLE_ON
                        : HapticFeedbackConstants.VIRTUAL_KEY;
            case TOGGLE_OFF:
                return sdk >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE ? HapticFeedbackConstants.TOGGLE_OFF
                        : HapticFeedbackConstants.VIRTUAL_KEY;
            default:
                return HapticFeedbackConstants.VIRTUAL_KEY;
        }
    }

    public static void perform(View view, Kind kind) {
        view.performHapticFeedback(constant(kind, Build.VERSION.SDK_INT));
    }

    public static void toggle(View view, boolean on) {
        perform(view, on ? Kind.TOGGLE_ON : Kind.TOGGLE_OFF);
    }
}

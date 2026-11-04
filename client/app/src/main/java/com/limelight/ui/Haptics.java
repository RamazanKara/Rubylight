package com.limelight.ui;

import android.annotation.SuppressLint;
import android.os.Build;
import android.view.HapticFeedbackConstants;
import android.view.View;

/**
 * Touch feedback for the app's own controls: the PC keyboard's keys, the trackpads, the WASD stick, slider steps,
 * settings switches and the shell bar. View.performHapticFeedback follows the system's touch feedback setting; the
 * stream's on-screen controls also follow Vibrate on press ({@code checkbox_vibrate_osc}).
 * Each kind takes the newest constant the device knows and the closest older one below it.
 */
public final class Haptics {
    public enum Kind {
        /** A key of an on-screen keyboard going down. */
        KEY,
        /** A trackpad tap or an on-screen mouse button going down. */
        CLICK,
        /** A WASD stick entering one direction: W, A, S or D. */
        DIRECTION,
        /** A WASD stick entering a diagonal, two keys at once; it must feel unlike {@link #DIRECTION}. */
        DIAGONAL,
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
            case DIRECTION:
                // One light tick for one key; the clock tick before Android 6.
                return sdk >= Build.VERSION_CODES.M ? HapticFeedbackConstants.CONTEXT_CLICK
                        : HapticFeedbackConstants.CLOCK_TICK;
            case DIAGONAL:
                // Two keys: a full click, firmer than the direction's tick. Android 11's confirm plays the same click.
                return HapticFeedbackConstants.VIRTUAL_KEY;
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

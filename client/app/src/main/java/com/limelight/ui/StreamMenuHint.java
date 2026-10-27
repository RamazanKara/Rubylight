package com.limelight.ui;

import android.content.Context;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.provider.Settings;
import android.view.InputDevice;

import com.limelight.R;

import java.util.ArrayList;
import java.util.List;

/**
 * The one-time line on the first stream that tells how to open the stream menu with this device's
 * own inputs: the Back gesture or button, a controller's menu combination, a keyboard's shortcut.
 */
public final class StreamMenuHint {
    static final String PREFERENCES = "FirstRun";
    static final String SEEN = "stream_menu_hint_seen";
    /** How long the hint stays before it counts as read and goes. */
    public static final long SHOW_MS = 10000;

    /** How this device reaches Back while a stream hides the system bars. */
    public enum Back { GESTURE, BUTTONS, REMOTE }

    private StreamMenuHint() {
    }

    public static boolean seen(Context context) {
        return preferences(context).getBoolean(SEEN, false);
    }

    public static void markSeen(Context context) {
        preferences(context).edit().putBoolean(SEEN, true).apply();
    }

    private static SharedPreferences preferences(Context context) {
        return context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE);
    }

    /** The ways to open the menu, as string resources, the device's Back first. */
    static List<Integer> methods(Back back, boolean controller, boolean keyboard) {
        List<Integer> methods = new ArrayList<>();
        methods.add(back == Back.GESTURE ? R.string.stream_menu_hint_swipe :
                back == Back.BUTTONS ? R.string.stream_menu_hint_buttons : R.string.stream_menu_hint_back);
        if (controller) methods.add(R.string.stream_menu_hint_controller);
        if (keyboard) methods.add(R.string.stream_menu_hint_keyboard);
        return methods;
    }

    /** "To open the stream menu, swipe back from the screen edge or press Start + Back + LB + RB on the controller." */
    public static String text(Context context, Back back, boolean controller, boolean keyboard) {
        List<Integer> methods = methods(back, controller, keyboard);
        String joined = context.getString(methods.get(0));
        for (int i = 1; i < methods.size(); i++) {
            String next = context.getString(methods.get(i));
            joined = i == methods.size() - 1 ? context.getString(R.string.stream_menu_hint_or, joined, next) :
                    context.getString(R.string.stream_menu_hint_list, joined, next);
        }
        return context.getString(R.string.stream_menu_hint, joined);
    }

    /** The hint for the inputs this device has right now. */
    public static String text(Context context, boolean controller) {
        return text(context, back(context), controller, hasKeyboard());
    }

    static Back back(Context context) {
        if (!context.getPackageManager().hasSystemFeature(PackageManager.FEATURE_TOUCHSCREEN)) return Back.REMOTE;
        int mode = -1;
        try {
            // Android 10 added gesture navigation and this setting with it.
            mode = Settings.Secure.getInt(context.getContentResolver(), "navigation_mode", -1);
        } catch (SecurityException ignored) {
        }
        // 2 is gesture navigation; 0 and 1 show Back as a button once the bar is swiped in.
        return mode == 2 ? Back.GESTURE : Back.BUTTONS;
    }

    /**
     * Top centre of the stream, below a camera cutout the parent's padding leaves uncovered, and below
     * the performance overlay when that sits at the top too and shares the hint's columns.
     */
    public static void place(android.view.View hint, android.view.View overlay) {
        android.view.View parent = (android.view.View) hint.getParent();
        androidx.core.view.WindowInsetsCompat insets = androidx.core.view.ViewCompat.getRootWindowInsets(parent);
        int cutout = insets == null ? 0 :
                insets.getInsets(androidx.core.view.WindowInsetsCompat.Type.displayCutout()).top;
        int margin = Math.round(16 * hint.getResources().getDisplayMetrics().density);
        int top = Math.max(0, cutout - parent.getPaddingTop()) + margin;
        android.widget.FrameLayout.LayoutParams overlayParams =
                (android.widget.FrameLayout.LayoutParams) overlay.getLayoutParams();
        // Before the first layout the hint's columns are unknown; a wide screen still centres it over a
        // top-start overlay, so only columns known to be apart keep it at the top.
        boolean sharesColumns = hint.getWidth() == 0 || overlay.getWidth() == 0 ||
                (hint.getLeft() < overlay.getRight() && overlay.getLeft() < hint.getRight());
        if (overlay.getVisibility() == android.view.View.VISIBLE && sharesColumns &&
                (overlayParams.gravity & android.view.Gravity.VERTICAL_GRAVITY_MASK) == android.view.Gravity.TOP) {
            top = overlayParams.topMargin + overlay.getHeight() + margin / 2;
        }
        android.widget.FrameLayout.LayoutParams params = (android.widget.FrameLayout.LayoutParams) hint.getLayoutParams();
        if (params.topMargin != top) {
            params.topMargin = top;
            hint.setLayoutParams(params);
        }
    }

    /** A gamepad that Android reports; pads the USB driver owns are counted by the stream. */
    public static boolean hasGamepad() {
        for (int id : InputDevice.getDeviceIds()) {
            InputDevice device = InputDevice.getDevice(id);
            if (device == null || device.isVirtual()) continue;
            int sources = device.getSources();
            if ((sources & InputDevice.SOURCE_GAMEPAD) == InputDevice.SOURCE_GAMEPAD ||
                    (sources & InputDevice.SOURCE_JOYSTICK) == InputDevice.SOURCE_JOYSTICK) {
                return true;
            }
        }
        return false;
    }

    static boolean hasKeyboard() {
        for (int id : InputDevice.getDeviceIds()) {
            InputDevice device = InputDevice.getDevice(id);
            if (device != null && !device.isVirtual() && device.getKeyboardType() == InputDevice.KEYBOARD_TYPE_ALPHABETIC &&
                    (android.os.Build.VERSION.SDK_INT < android.os.Build.VERSION_CODES.Q || device.isExternal())) {
                return true;
            }
        }
        return false;
    }
}

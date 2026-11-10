package com.limelight.ui;

import android.app.Activity;
import android.app.Dialog;
import android.os.Build;
import android.view.Gravity;
import android.view.Window;
import android.view.WindowManager;

import androidx.annotation.RequiresApi;

import com.limelight.R;

import java.util.function.Consumer;

/**
 * The one window the stream menu may add: an empty one under the panel, exactly its size, that asks the system to blur what is
 * behind it. It exists only while the menu is open: {@link #close} removes it, so a stream with no menu carries nothing. It takes
 * no touch and no focus, and stands below the panel because it is shown first. Which tier allows it comes from
 * {@link GlassTier#land} and the system's own switch, never from a device's name.
 */
final class MenuBlurWindow {
    /** For a test: forces what the system answers about cross-window blur. */
    static Boolean enabledForTest;

    private final Dialog window;
    private final WindowManager manager;
    private final Consumer<Boolean> listener;
    private boolean closed;

    /** The rule: an Android that has the call, a tier that can afford live glass, and the system's switch on right now. */
    static boolean allowed(int api, GlassTier tier, boolean systemSwitch) {
        return api >= Build.VERSION_CODES.S && (tier == GlassTier.LENS || tier == GlassTier.LIVE) && systemSwitch;
    }

    static boolean systemSwitch(Activity owner) {
        if (enabledForTest != null) return enabledForTest;
        return Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && owner.getWindowManager().isCrossWindowBlurEnabled();
    }

    /** The window, shown and still empty, or null when this device cannot blur now. {@code onSwitch} hears the system turn it off and on. */
    static MenuBlurWindow openIfAllowed(Activity owner, int radius, Consumer<Boolean> onSwitch) {
        if (!allowed(Build.VERSION.SDK_INT, GlassTier.land(owner), systemSwitch(owner))) return null;
        return new MenuBlurWindow(owner, radius, onSwitch);
    }

    private MenuBlurWindow(Activity owner, int radius, Consumer<Boolean> onSwitch) {
        manager = owner.getWindowManager();
        window = new Dialog(owner, R.style.StreamMenuBlurWindow);
        window.setCancelable(false);
        Window w = window.getWindow();
        w.addFlags(WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE | WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE
                | WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS);
        w.clearFlags(WindowManager.LayoutParams.FLAG_DIM_BEHIND);
        if (Motion.reduced(owner)) w.setWindowAnimations(0);
        WindowManager.LayoutParams params = w.getAttributes();
        params.gravity = Gravity.TOP | Gravity.START;
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            params.layoutInDisplayCutoutMode = WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS;
            params.setFitInsetsTypes(0);
        }
        params.x = 0;
        params.y = 0;
        // Nothing to see until the panel has a size.
        params.width = 1;
        params.height = 1;
        w.setAttributes(params);
        listener = onSwitch == null ? null : onSwitch::accept;
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) applyBlur(w, radius);
        window.show();
    }

    @RequiresApi(Build.VERSION_CODES.S)
    private void applyBlur(Window w, int radius) {
        // The window keeps its radius on its decor view, which exists only once asked for.
        w.getDecorView();
        w.setBackgroundBlurRadius(radius);
        if (listener != null) manager.addCrossWindowBlurEnabledListener(listener);
    }

    /** Lies behind the panel at these screen pixels; a new page moves it with the panel's height. */
    void cover(int x, int y, int width, int height) {
        if (closed || width <= 0 || height <= 0) return;
        Window w = window.getWindow();
        WindowManager.LayoutParams params = w.getAttributes();
        if (params.x == x && params.y == y && params.width == width && params.height == height) return;
        params.x = x;
        params.y = y;
        params.width = width;
        params.height = height;
        w.setAttributes(params);
    }

    /** Removes the window, not hides it, and stops listening. */
    void close() {
        if (closed) return;
        closed = true;
        if (listener != null && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) manager.removeCrossWindowBlurEnabledListener(listener);
        window.dismiss();
    }

    boolean isShowing() {
        return !closed && window.isShowing();
    }
}

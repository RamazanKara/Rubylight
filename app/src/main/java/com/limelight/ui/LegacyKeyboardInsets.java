package com.limelight.ui;

import android.content.Context;
import android.graphics.Color;
import android.graphics.Rect;
import android.graphics.drawable.ColorDrawable;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.view.ViewTreeObserver;
import android.view.WindowManager;
import android.widget.PopupWindow;

import com.limelight.LimeLog;

/**
 * The soft keyboard on Android 10 and older, for a window that takes no room from it
 * (adjustNothing): the system tells such a window nothing of the keyboard before Android 11, so an
 * invisible popup as tall as the window, which does resize for it (adjustResize), shows how much of
 * the window's bottom it covers. It takes no touches, no focus and no room on the screen.
 */
public final class LegacyKeyboardInsets implements ViewTreeObserver.OnGlobalLayoutListener {
    /** Less of the window than this is a bar (the navigation bar), not a keyboard. */
    public static final float MIN_KEYBOARD_FRACTION = 0.15f;

    /** On the UI thread, each time the keyboard shows, hides or changes height. */
    public interface Listener {
        /** The keyboard covers {@code bottom} px of the content's bottom, 0 when hidden. */
        void onKeyboard(boolean visible, int bottom);
    }

    private final View content;
    private final Listener listener;
    private final View probe;
    private final PopupWindow popup;
    private final Rect frame = new Rect();
    private final int[] location = new int[2];
    private final Runnable show = this::show;
    private boolean started;
    private boolean visible;
    private int bottom;

    public LegacyKeyboardInsets(Context context, View content, Listener listener) {
        this.content = content;
        this.listener = listener;
        probe = new View(context);
        popup = new PopupWindow(probe, 0, ViewGroup.LayoutParams.MATCH_PARENT, false);
        popup.setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE);
        popup.setInputMethodMode(PopupWindow.INPUT_METHOD_NEEDED);
        popup.setTouchable(false);
        popup.setBackgroundDrawable(new ColorDrawable(Color.TRANSPARENT));
    }

    /** Follows the keyboard from when the content is in a window until {@link #stop()}. */
    public void start() {
        started = true;
        show();
    }

    public void stop() {
        started = false;
        content.removeCallbacks(show);
        probe.getViewTreeObserver().removeOnGlobalLayoutListener(this);
        if (popup.isShowing()) popup.dismiss();
    }

    /** Whether the keyboard covers the content's bottom, as last measured. */
    public boolean visible() {
        return visible;
    }

    private void show() {
        if (!started || popup.isShowing()) return;
        if (content.getWindowToken() == null) {
            // Runs once the content is attached.
            content.post(show);
            return;
        }
        probe.getViewTreeObserver().addOnGlobalLayoutListener(this);
        try {
            popup.showAtLocation(content, Gravity.NO_GRAVITY, 0, 0);
        } catch (WindowManager.BadTokenException e) {
            probe.getViewTreeObserver().removeOnGlobalLayoutListener(this);
            LimeLog.warning("The stream cannot follow the soft keyboard: " + e.getMessage());
        }
    }

    @Override
    public void onGlobalLayout() {
        probe.getWindowVisibleDisplayFrame(frame);
        content.getLocationOnScreen(location);
        onFrame(location[1] + content.getHeight(), frame.bottom, content.getRootView().getHeight());
    }

    /** The probe's visible frame ends at {@code visibleBottom}, the content at {@code contentBottom}, on screen. */
    void onFrame(int contentBottom, int visibleBottom, int windowHeight) {
        int covered = covered(contentBottom, visibleBottom, windowHeight);
        boolean shown = covered > 0;
        if (shown == visible && covered == bottom) return;
        visible = shown;
        bottom = covered;
        listener.onKeyboard(shown, covered);
    }

    /**
     * How much of the content's bottom, at {@code contentBottom} on screen, a keyboard covers when
     * the resizing window's visible part ends at {@code visibleBottom}: 0 for less than
     * {@link #MIN_KEYBOARD_FRACTION} of {@code windowHeight}, which is a bar or no keyboard.
     */
    public static int covered(int contentBottom, int visibleBottom, int windowHeight) {
        int covered = contentBottom - visibleBottom;
        return windowHeight > 0 && covered > windowHeight * MIN_KEYBOARD_FRACTION ? covered : 0;
    }
}

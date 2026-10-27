package com.limelight.ui;

import android.app.Activity;
import android.view.KeyEvent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.LinearLayout;

import com.limelight.R;

/**
 * D-pad and controller focus between a screen's content and its shell bar. Up from the top of the content lands on the
 * bar's current tab (the focus search alone picks the tab nearest across, which can be another one), and Down from the
 * bar is the focus search's own move into the content.
 */
public final class ShellFocus {
    private ShellFocus() {
    }

    /**
     * Handles an Up press that would leave {@code content} for the shell bar by focusing the bar's current tab.
     * Call it from {@code dispatchKeyEvent}; true when it moved focus and the press is used up.
     */
    public static boolean upToCurrentTab(Activity activity, KeyEvent event, View content) {
        if (event.getAction() != KeyEvent.ACTION_DOWN || event.getKeyCode() != KeyEvent.KEYCODE_DPAD_UP
                || !event.hasNoModifiers()) {
            return false;
        }
        // The content's own focus, so a window that is not focused yet (or a test) behaves like a live one.
        View focused = content.findFocus();
        View bar = activity.findViewById(R.id.shell_bar);
        if (focused == null || bar == null) {
            return false;
        }
        View next = focused.focusSearch(View.FOCUS_UP);
        if (next == null || !isInside(bar, next)) {
            return false;
        }
        View current = currentTab(bar);
        return current != null && current.requestFocus();
    }

    /** The bar's tab for the screen shown: selected and on screen, else null. */
    static View currentTab(View bar) {
        LinearLayout items = bar.findViewById(R.id.shell_bar_items);
        for (int i = 0; items != null && i < items.getChildCount(); i++) {
            View tab = items.getChildAt(i);
            if (tab.isSelected() && tab.getVisibility() == View.VISIBLE && tab.isFocusable()) return tab;
        }
        return null;
    }

    private static boolean isInside(View parent, View child) {
        for (View view = child; view != null; ) {
            if (view == parent) return true;
            view = view.getParent() instanceof ViewGroup ? (View) view.getParent() : null;
        }
        return false;
    }
}

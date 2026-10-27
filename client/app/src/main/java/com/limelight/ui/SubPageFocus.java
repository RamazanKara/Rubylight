package com.limelight.ui;

import android.app.Activity;
import android.content.res.Configuration;
import android.os.Build;
import android.view.View;
import android.view.ViewGroup;
import android.widget.AbsListView;
import android.widget.EditText;
import android.widget.ScrollView;

import androidx.appcompat.widget.SearchView;

import com.limelight.R;

import java.util.ArrayList;

/**
 * The focus rules every Settings sub-page shares (Profiles, Controllers, Keyboard shortcuts, GPU driver, Latency
 * benchmark): the first focus lands on the content, never on the toolbar, and every focusable row or button shows
 * the same outline when it has focus. Pages call {@link #apply} once their views exist and again after each rebuild.
 */
public final class SubPageFocus {
    private SubPageFocus() {
    }

    /** Whether the page is driven by a remote, controller or keyboard rather than by touch. */
    static boolean directional(Activity activity, View content) {
        return (activity.getResources().getConfiguration().uiMode & Configuration.UI_MODE_TYPE_MASK)
                == Configuration.UI_MODE_TYPE_TELEVISION || !content.isInTouchMode();
    }

    /**
     * Outlines the focusables under {@code content}, and moves focus into it when the page is not touch-driven and
     * focus is missing or elsewhere: at the start, and again after a rebuild that removed the view that had it.
     */
    public static void apply(Activity activity, View content) {
        decorate(content);
        content.post(() -> focusFirst(activity, content));
    }

    /** Gives the first focusable of {@code content} focus unless something in it already has it. */
    static void focusFirst(Activity activity, View content) {
        if (activity.isFinishing() || activity.isDestroyed() || !directional(activity, content)) return;
        // The view tree knows its focus before the window reports it.
        View current = activity.findViewById(android.R.id.content).findFocus();
        if (current != null && within(content, current)) return;
        ArrayList<View> candidates = new ArrayList<>();
        content.addFocusables(candidates, View.FOCUS_DOWN, View.FOCUSABLES_ALL);
        for (View candidate : candidates) {
            if (candidate.isEnabled() && candidate.getVisibility() == View.VISIBLE
                    && (candidate.requestFocus() || candidate.requestFocusFromTouch())) {
                return;
            }
        }
    }

    private static boolean within(View ancestor, View view) {
        for (View v = view; v != null; v = v.getParent() instanceof View ? (View) v.getParent() : null) {
            if (v == ancestor) return true;
        }
        return false;
    }

    /** Gives each focusable row and button the focus outline; lists and fields draw their own. */
    static void decorate(View view) {
        if (view instanceof ViewGroup && !(view instanceof AbsListView)) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) decorate(group.getChildAt(i));
        }
        if (!view.isFocusable() || view instanceof AbsListView || view instanceof EditText
                || view instanceof ScrollView || view instanceof SearchView || view.getTag(R.id.focus_ring) != null) {
            return;
        }
        view.setTag(R.id.focus_ring, Boolean.TRUE);
        // Before API 23 a view has no foreground; those versions keep the Material state layer.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M && view.getForeground() == null) {
            view.setForeground(view.getContext().getDrawable(R.drawable.focus_ring_row));
        }
    }
}

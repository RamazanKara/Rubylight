package com.limelight.preferences;

import android.animation.ValueAnimator;
import android.app.Activity;
import android.graphics.Rect;
import android.view.View;
import android.view.ViewGroup;
import android.view.ViewTreeObserver;
import android.view.animation.AnimationUtils;
import android.view.animation.Interpolator;
import android.widget.ListView;

import com.limelight.R;
import com.limelight.ui.Motion;

/**
 * The motion of TV Settings, which is all it has: the amber bar that marks the current section on the rail, and the right
 * pane coming in when the section changes. The bar is one view over the rail (layout/activity_stream_settings_tv, which
 * TVs get as activity_stream_settings): it slides to a new section's row over 160 ms and follows the list's scrolling at once. Nothing moves under Reduce motion;
 * the bar and the panes are still there. Focus is not touched here (PaneFocus and StreamSettings own it).
 */
final class SettingsRail implements ViewTreeObserver.OnPreDrawListener {
    static final long BAR_MS = 160;
    static final long PANE_MS = 140;
    static final float PANE_SHIFT_DP = 16;

    /** The rail's sections, by key, as their icons. */
    static int iconOf(String key) {
        switch (key) {
            case "video": return R.drawable.ic_play;
            case "display": return R.drawable.ic_monitor;
            case "input": return R.drawable.ic_gamepad;
            case "audio": return R.drawable.ic_speaker;
            case "streaming": return R.drawable.ic_network;
            case "app": return R.drawable.ic_info;
            case "advanced": return R.drawable.ic_tune;
            default: return 0;
        }
    }

    private final Activity activity;
    private final ViewGroup rail;
    private final View bar;
    private final View pane;
    private ListView list;
    private int position = -1;
    private boolean glide;
    private float target = Float.NaN;
    private ValueAnimator slide;
    private String paneSection;
    private final Rect scratch = new Rect();

    /** Null when the layout has no rail (a phone, or a wide screen that is not a TV). */
    static SettingsRail attach(Activity activity) {
        ViewGroup rail = activity.findViewById(R.id.settings_rail);
        View bar = activity.findViewById(R.id.settings_rail_bar);
        View pane = activity.findViewById(R.id.stream_settings);
        return rail == null || bar == null || pane == null ? null : new SettingsRail(activity, rail, bar, pane);
    }

    private SettingsRail(Activity activity, ViewGroup rail, View bar, View pane) {
        this.activity = activity;
        this.rail = rail;
        this.bar = bar;
        this.pane = pane;
        rail.getViewTreeObserver().addOnPreDrawListener(this);
    }

    /** The rail's list, once its fragment has a view. */
    void setList(ListView list) {
        this.list = list;
    }

    /** The current section is now at {@code position} in the rail's list; the bar slides there unless this is its first place. */
    void mark(int position) {
        if (position == this.position) return;
        glide = this.position >= 0 && bar.getVisibility() == View.VISIBLE && !Motion.reduced(activity);
        this.position = position;
        rail.invalidate();
    }

    /** The right pane is about to show {@code section}: it comes in from a little to the side unless it showed it already. */
    void enterPane(String section) {
        String previous = paneSection;
        paneSection = section;
        if (previous == null || previous.equals(section) || Motion.reduced(activity)) {
            pane.animate().cancel();
            pane.setAlpha(1f);
            pane.setTranslationX(0f);
            return;
        }
        Interpolator interpolator = AnimationUtils.loadInterpolator(activity, R.interpolator.tv_emph_dec);
        pane.animate().cancel();
        pane.setAlpha(0f);
        pane.setTranslationX(PANE_SHIFT_DP * activity.getResources().getDisplayMetrics().density);
        pane.animate().alpha(1f).translationX(0f).setDuration(PANE_MS).setInterpolator(interpolator).start();
    }

    /** Where the bar should be: centred on the current row, in the rail's coordinates, or NaN when that row is not on screen. */
    float targetY() {
        if (list == null || position < 0 || !list.isAttachedToWindow()) return Float.NaN;
        View row = list.getChildAt(position - list.getFirstVisiblePosition());
        if (row == null) return Float.NaN;
        scratch.set(0, 0, row.getWidth(), row.getHeight());
        rail.offsetDescendantRectToMyCoords(row, scratch);
        float y = scratch.top + (row.getHeight() - bar.getHeight()) / 2f;
        // A row half scrolled out of the list keeps the bar inside the list.
        scratch.set(0, 0, list.getWidth(), list.getHeight());
        rail.offsetDescendantRectToMyCoords(list, scratch);
        if (y < scratch.top || y + bar.getHeight() > scratch.bottom) return Float.NaN;
        return y;
    }

    @Override
    public boolean onPreDraw() {
        float y = targetY();
        if (Float.isNaN(y)) {
            if (slide == null || !slide.isRunning()) bar.setVisibility(View.INVISIBLE);
            return true;
        }
        if (y == target && bar.getVisibility() == View.VISIBLE) return true;
        boolean animate = glide && !Float.isNaN(target);
        glide = false;
        target = y;
        if (slide != null) slide.cancel();
        bar.setVisibility(View.VISIBLE);
        if (!animate) {
            bar.setTranslationY(y);
            return true;
        }
        slide = ValueAnimator.ofFloat(bar.getTranslationY(), y);
        slide.setDuration(BAR_MS);
        slide.setInterpolator(AnimationUtils.loadInterpolator(activity, R.interpolator.tv_std));
        slide.addUpdateListener(animation -> bar.setTranslationY((float) animation.getAnimatedValue()));
        slide.start();
        return true;
    }

    /** The screen is going away. */
    void detach() {
        if (slide != null) slide.cancel();
        pane.animate().cancel();
        if (rail.getViewTreeObserver().isAlive()) rail.getViewTreeObserver().removeOnPreDrawListener(this);
    }

    int position() {
        return position;
    }

    boolean sliding() {
        return slide != null && slide.isRunning();
    }
}

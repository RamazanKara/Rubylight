package com.limelight.ui;

import android.animation.AnimatorInflater;
import android.animation.StateListAnimator;
import android.animation.ValueAnimator;
import android.content.Context;
import android.os.Build;
import android.preference.PreferenceManager;
import android.provider.Settings;
import android.util.TypedValue;

import com.limelight.R;

/**
 * The one answer to "should this screen animate": the Reduce motion setting, and under Follow system the
 * system's animation scale. Reduced means no zoom or slide, only a state change; a focus ring still shows.
 * Screens ask {@link #reduced}; resources follow it through the theme (see AppTheme.apply).
 */
public final class Motion {
    public static final String REDUCE_MOTION = "list_reduce_motion";
    public static final String FOLLOW_SYSTEM = "system";
    public static final String REDUCE = "reduce";
    public static final String FULL = "full";

    private Motion() { }

    /** The stored setting; anything unknown counts as Follow system. */
    public static String setting(Context context) {
        String value = PreferenceManager.getDefaultSharedPreferences(context).getString(REDUCE_MOTION, FOLLOW_SYSTEM);
        return REDUCE.equals(value) || FULL.equals(value) ? value : FOLLOW_SYSTEM;
    }

    public static boolean reduced(Context context) {
        return decide(setting(context), systemAnimatorScale(context), animatorsEnabled());
    }

    /**
     * Follow system reduces motion when the animation scale is 0 or animators are off for the app (battery
     * saver, a test harness); Reduce and Full ignore the system.
     */
    static boolean decide(String setting, float animatorScale, boolean animatorsEnabled) {
        if (REDUCE.equals(setting)) return true;
        if (FULL.equals(setting)) return false;
        return animatorScale <= 0f || !animatorsEnabled;
    }

    private static float systemAnimatorScale(Context context) {
        return Settings.Global.getFloat(context.getContentResolver(), Settings.Global.ANIMATOR_DURATION_SCALE, 1f);
    }

    private static boolean animatorsEnabled() {
        return Build.VERSION.SDK_INT < Build.VERSION_CODES.O || ValueAnimator.areAnimatorsEnabled();
    }

    /**
     * The focus animator of a control: the theme's (high contrast makes the ring thicker, reduced motion drops
     * the zoom), and the still one when the stream's theme, which has no overlays, is asked under Reduce motion.
     */
    public static StateListAnimator controlFocus(Context context) {
        TypedValue value = new TypedValue();
        int id = context.getTheme().resolveAttribute(R.attr.nightjarControlFocusAnimator, value, true) &&
                value.resourceId != 0 ? value.resourceId : R.animator.tv_control_focus;
        if (id == R.animator.tv_control_focus && reduced(context)) id = R.animator.tv_control_focus_static;
        return AnimatorInflater.loadStateListAnimator(context, id);
    }
}

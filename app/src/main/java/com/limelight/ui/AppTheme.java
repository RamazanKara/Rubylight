package com.limelight.ui;

import android.app.Activity;
import android.content.Context;
import android.graphics.drawable.Drawable;
import android.preference.PreferenceManager;

import com.google.android.material.color.MaterialColors;
import com.google.android.material.shape.MaterialShapeDrawable;
import com.limelight.R;

/** App appearance is independent of stream profiles and the system's day/night setting. */
public final class AppTheme {
    public static final String OLED_BLACK = "checkbox_oled_black";
    public static final String ALWAYS_DARK = "checkbox_always_dark";

    private AppTheme() { }

    public static boolean isOledBlack(Context context) {
        return PreferenceManager.getDefaultSharedPreferences(context).getBoolean(OLED_BLACK, false);
    }

    /** The night palette whatever the system says; OLED black is a darker variant of it. */
    public static boolean isDark(Context context) {
        return isOledBlack(context) ||
                PreferenceManager.getDefaultSharedPreferences(context).getBoolean(ALWAYS_DARK, false);
    }

    /** Theme for the app's own screens. The stream activity is always dark and only takes OLED black. */
    public static void apply(Activity activity) {
        if (isOledBlack(activity)) activity.setTheme(R.style.OledBlackTheme);
        else if (isDark(activity) && !(activity instanceof StreamWindow)) activity.setTheme(R.style.DarkTheme);
    }

    /**
     * On black, a dialog or sheet has no edge against the screen behind it. Give its surface a thin
     * outline instead of lifting it to grey.
     */
    public static void outlineOnBlack(Context context, Drawable background) {
        if (!(background instanceof MaterialShapeDrawable) || !isOledBlack(context)) return;
        float width = context.getResources().getDisplayMetrics().density;
        ((MaterialShapeDrawable) background).setStroke(width, MaterialColors.getColor(context,
                com.google.android.material.R.attr.colorOutlineVariant, 0));
    }
}

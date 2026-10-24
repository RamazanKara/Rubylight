package com.limelight.ui;

import android.app.Activity;
import android.content.Context;
import android.content.SharedPreferences;
import android.content.res.Configuration;
import android.graphics.drawable.Drawable;
import android.preference.PreferenceManager;

import com.google.android.material.color.MaterialColors;
import com.google.android.material.shape.MaterialShapeDrawable;
import com.limelight.R;

/** App appearance is independent of stream profiles and the system's day/night setting. */
public final class AppTheme {
    public static final String OLED_BLACK = "checkbox_oled_black";
    public static final String ALWAYS_DARK = "checkbox_always_dark";
    public static final String HIGH_CONTRAST = "checkbox_high_contrast";
    /** The app's interface size in percent, stored as a string like the other list settings. */
    public static final String UI_SCALE = "list_ui_scale";
    public static final int DEFAULT_UI_SCALE = 100;
    /** The steps of the Interface size setting; arrays.xml lists the same values. */
    static final int[] UI_SCALES = {85, 100, 115, 130, 150};

    private AppTheme() { }

    /** Black surfaces are in effect: the OLED black setting, or high contrast, which is always black. */
    public static boolean isOledBlack(Context context) {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
        return prefs.getBoolean(OLED_BLACK, false) || prefs.getBoolean(HIGH_CONTRAST, false);
    }

    public static boolean isHighContrast(Context context) {
        return PreferenceManager.getDefaultSharedPreferences(context).getBoolean(HIGH_CONTRAST, false);
    }

    /** The chosen interface size in percent; a value that is not one of the steps counts as the default. */
    public static int uiScalePercent(Context context) {
        return parseScale(PreferenceManager.getDefaultSharedPreferences(context).getString(UI_SCALE, null));
    }

    static int parseScale(String value) {
        if (value == null) return DEFAULT_UI_SCALE;
        try {
            int percent = Integer.parseInt(value.trim());
            for (int step : UI_SCALES) if (step == percent) return percent;
        } catch (NumberFormatException ignored) {
            // Falls through to the default.
        }
        return DEFAULT_UI_SCALE;
    }

    /**
     * The context with the density multiplied by the interface size, so a screen's layout scales as a whole and
     * not only its text; the system font size still applies on top. The width and height in dp shrink to match,
     * which is what picks the layout qualifiers (a phone at 150 % lays out like a narrower phone).
     */
    public static Context scaled(Context context) {
        int percent = uiScalePercent(context);
        if (percent == DEFAULT_UI_SCALE) return context;
        Configuration config = new Configuration(context.getResources().getConfiguration());
        if (config.densityDpi == Configuration.DENSITY_DPI_UNDEFINED) {
            config.densityDpi = context.getResources().getDisplayMetrics().densityDpi;
        }
        float factor = percent / 100f;
        config.densityDpi = Math.round(config.densityDpi * factor);
        if (config.screenWidthDp != Configuration.SCREEN_WIDTH_DP_UNDEFINED) config.screenWidthDp = Math.round(config.screenWidthDp / factor);
        if (config.screenHeightDp != Configuration.SCREEN_HEIGHT_DP_UNDEFINED) config.screenHeightDp = Math.round(config.screenHeightDp / factor);
        if (config.smallestScreenWidthDp != Configuration.SMALLEST_SCREEN_WIDTH_DP_UNDEFINED) {
            config.smallestScreenWidthDp = Math.round(config.smallestScreenWidthDp / factor);
        }
        return context.createConfigurationContext(config);
    }

    /** Everything that changes a screen's look and needs it recreated; compared before and after a settings change. */
    static String appearance(Context context) {
        return isOledBlack(context) + "/" + isDark(context) + "/" + isHighContrast(context) + "/" +
                uiScalePercent(context) + "/" + Motion.reduced(context);
    }

    static boolean affectsAppearance(String key) {
        return key == null || OLED_BLACK.equals(key) || ALWAYS_DARK.equals(key) || HIGH_CONTRAST.equals(key) ||
                UI_SCALE.equals(key) || Motion.REDUCE_MOTION.equals(key);
    }

    /** The night palette whatever the system says; OLED black is a darker variant of it. */
    public static boolean isDark(Context context) {
        return isOledBlack(context) ||
                PreferenceManager.getDefaultSharedPreferences(context).getBoolean(ALWAYS_DARK, false);
    }

    /** Theme for the app's own screens. The stream activity is always dark and only takes black surfaces. */
    public static void apply(Activity activity) {
        if (isOledBlack(activity)) activity.setTheme(R.style.OledBlackTheme);
        else if (isDark(activity) && !(activity instanceof StreamWindow)) activity.setTheme(R.style.DarkTheme);
        // The stream and its menu keep their own look; the overlays are for the app's screens.
        if (activity instanceof StreamWindow) return;
        boolean highContrast = isHighContrast(activity);
        boolean reduced = Motion.reduced(activity);
        if (highContrast) activity.getTheme().applyStyle(R.style.ThemeOverlay_Nightjar_HighContrast, true);
        if (reduced) {
            activity.getTheme().applyStyle(highContrast ? R.style.ThemeOverlay_Nightjar_HighContrast_ReducedMotion :
                    R.style.ThemeOverlay_Nightjar_ReducedMotion, true);
        }
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

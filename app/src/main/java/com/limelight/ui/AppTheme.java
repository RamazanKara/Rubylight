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
    /**
     * Which interface the app lays out: the device's own, or the TV or handheld one whatever the device is. A handheld
     * docked to a TV wants the TV layouts; every TV check in the app reads the Configuration's uiMode type.
     */
    public static final String UI_MODE = "list_ui_mode";
    public static final String UI_MODE_AUTO = "auto";
    public static final String UI_MODE_TV = "tv";
    public static final String UI_MODE_HANDHELD = "handheld";

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

    /** The chosen interface: automatic, tv or handheld; anything else counts as automatic. */
    public static String uiMode(Context context) {
        return parseUiMode(PreferenceManager.getDefaultSharedPreferences(context).getString(UI_MODE, null));
    }

    /**
     * The TV layout is forced on a device that is not a TV. A TV has no system bars, so the screens go immersive to
     * fill the viewport as on one. The device's own type comes from the UiModeManager, not from a Configuration,
     * which this setting overrides.
     */
    public static boolean forcedTelevision(Context context) {
        if (!UI_MODE_TV.equals(uiMode(context))) return false;
        android.app.UiModeManager manager = (android.app.UiModeManager) context.getApplicationContext()
                .getSystemService(Context.UI_MODE_SERVICE);
        return manager == null || manager.getCurrentModeType() != Configuration.UI_MODE_TYPE_TELEVISION;
    }

    /** Hides the status and navigation bars; a swipe from the edge shows them for a moment. */
    public static void hideSystemBars(Activity activity) {
        androidx.core.view.WindowInsetsControllerCompat controller = androidx.core.view.WindowCompat
                .getInsetsController(activity.getWindow(), activity.getWindow().getDecorView());
        controller.setSystemBarsBehavior(
                androidx.core.view.WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
        controller.hide(androidx.core.view.WindowInsetsCompat.Type.systemBars());
    }

    static String parseUiMode(String value) {
        return UI_MODE_TV.equals(value) || UI_MODE_HANDHELD.equals(value) ? value : UI_MODE_AUTO;
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
     * The context with the interface type and size applied. The density is multiplied by the interface size, so a
     * screen's layout scales as a whole and not only its text; the system font size still applies on top. The width
     * and height in dp shrink to match, which is what picks the layout qualifiers (a phone at 150 % lays out like a
     * narrower phone). The interface type replaces the uiMode type bits (and so picks the -television resources and
     * the TV checks) and keeps the night bits.
     */
    public static Context scaled(Context context) {
        int percent = uiScalePercent(context);
        String mode = uiMode(context);
        int type = UI_MODE_TV.equals(mode) ? Configuration.UI_MODE_TYPE_TELEVISION : Configuration.UI_MODE_TYPE_NORMAL;
        boolean retype = !UI_MODE_AUTO.equals(mode) &&
                (context.getResources().getConfiguration().uiMode & Configuration.UI_MODE_TYPE_MASK) != type;
        if (percent == DEFAULT_UI_SCALE && !retype) return context;
        Configuration config = new Configuration(context.getResources().getConfiguration());
        if (retype) config.uiMode = (config.uiMode & ~Configuration.UI_MODE_TYPE_MASK) | type;
        if (percent != DEFAULT_UI_SCALE) {
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
        }
        return context.createConfigurationContext(config);
    }

    /** Everything that changes a screen's look and needs it recreated; compared before and after a settings change. */
    static String appearance(Context context) {
        return isOledBlack(context) + "/" + isDark(context) + "/" + isHighContrast(context) + "/" +
                uiScalePercent(context) + "/" + uiMode(context) + "/" + Motion.reduced(context);
    }

    static boolean affectsAppearance(String key) {
        return key == null || OLED_BLACK.equals(key) || ALWAYS_DARK.equals(key) || HIGH_CONTRAST.equals(key) ||
                UI_SCALE.equals(key) || UI_MODE.equals(key) || Motion.REDUCE_MOTION.equals(key);
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
        // OledBlackTheme is the stream's too, so it names no window animation: the app's screens get theirs here.
        if (isOledBlack(activity)) activity.getTheme().applyStyle(R.style.ThemeOverlay_Nightjar_WindowAnimation, true);
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

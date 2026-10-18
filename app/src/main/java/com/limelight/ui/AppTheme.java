package com.limelight.ui;

import android.app.Activity;
import android.content.Context;
import android.preference.PreferenceManager;

import com.limelight.R;

/** App appearance is independent of stream profiles and the system's day/night setting. */
public final class AppTheme {
    public static final String OLED_BLACK = "checkbox_oled_black";

    private AppTheme() { }

    public static boolean isOledBlack(Context context) {
        return PreferenceManager.getDefaultSharedPreferences(context).getBoolean(OLED_BLACK, false);
    }

    public static void apply(Activity activity) {
        if (isOledBlack(activity)) activity.setTheme(R.style.OledBlackTheme);
    }
}

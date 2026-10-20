package com.limelight.ui;

import android.content.SharedPreferences;
import android.os.Bundle;
import android.preference.PreferenceManager;

import androidx.appcompat.app.AppCompatActivity;

/** Applies appearance before views or restored fragments are created, including on deep links. */
public abstract class NightjarActivity extends AppCompatActivity {
    private boolean oledBlack;
    private boolean dark;
    private boolean recreatingForTheme;
    private final SharedPreferences.OnSharedPreferenceChangeListener appearanceListener = (prefs, key) -> {
        // Android 11+ can report clear() as a null key when all settings are reset.
        if (key == null || AppTheme.OLED_BLACK.equals(key) || AppTheme.ALWAYS_DARK.equals(key)) refreshAppearance();
    };

    @Override
    protected void onCreate(Bundle state) {
        oledBlack = AppTheme.isOledBlack(this);
        dark = AppTheme.isDark(this);
        AppTheme.apply(this);
        super.onCreate(state);
    }

    @Override
    protected void onStart() {
        super.onStart();
        PreferenceManager.getDefaultSharedPreferences(this).registerOnSharedPreferenceChangeListener(appearanceListener);
    }

    @Override
    protected void onResume() {
        super.onResume();
        refreshAppearance();
    }

    @Override
    protected void onStop() {
        PreferenceManager.getDefaultSharedPreferences(this).unregisterOnSharedPreferenceChangeListener(appearanceListener);
        super.onStop();
    }

    private void refreshAppearance() {
        if (!recreatingForTheme && !isFinishing() && !isDestroyed() &&
                (oledBlack != AppTheme.isOledBlack(this) || dark != AppTheme.isDark(this))) {
            recreatingForTheme = true;
            recreate();
        }
    }
}

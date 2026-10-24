package com.limelight.ui;

import android.content.SharedPreferences;
import android.os.Bundle;
import android.preference.PreferenceManager;

import androidx.appcompat.app.AppCompatActivity;

/** Applies appearance before views or restored fragments are created, including on deep links. */
public abstract class NightjarActivity extends AppCompatActivity {
    @Override
    protected void attachBaseContext(android.content.Context base) {
        super.attachBaseContext(AppTheme.scaled(com.limelight.utils.UiHelper.localeContext(base)));
    }

    /** What the screen was themed with; see {@link AppTheme#appearance}. */
    private String appearance;
    private boolean recreatingForTheme;
    private final SharedPreferences.OnSharedPreferenceChangeListener appearanceListener = (prefs, key) -> {
        // Android 11+ can report clear() as a null key when all settings are reset.
        if (AppTheme.affectsAppearance(key)) refreshAppearance();
    };

    @Override
    protected void onCreate(Bundle state) {
        appearance = AppTheme.appearance(this);
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
    protected void onPause() {
        dpadEchoes.reset();
        super.onPause();
    }

    @Override
    protected void onStop() {
        PreferenceManager.getDefaultSharedPreferences(this).unregisterOnSharedPreferenceChangeListener(appearanceListener);
        super.onStop();
    }

    private final DpadEchoFilter dpadEchoes = new DpadEchoFilter();

    /** A pad that reports its D-pad as keys and as a hat must not move the focus twice. */
    @Override
    public boolean dispatchKeyEvent(android.view.KeyEvent event) {
        return dpadEchoes.drop(event) || super.dispatchKeyEvent(event);
    }

    private void refreshAppearance() {
        if (!recreatingForTheme && !isFinishing() && !isDestroyed() &&
                !AppTheme.appearance(this).equals(appearance)) {
            recreatingForTheme = true;
            recreate();
        }
    }
}

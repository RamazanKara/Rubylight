package com.limelight.ui;

import android.content.SharedPreferences;
import android.os.Bundle;
import android.preference.PreferenceManager;

import androidx.appcompat.app.AppCompatActivity;

import com.limelight.preferences.LinkGate;
import com.limelight.preferences.PairLinkActivity;

/** Applies appearance before views or restored fragments are created, including on deep links. */
public abstract class NightjarActivity extends AppCompatActivity {
    @Override
    protected void attachBaseContext(android.content.Context base) {
        android.content.Context localized = com.limelight.utils.UiHelper.localeContext(base);
        android.content.Context scaled = AppTheme.scaled(localized, compactOnTelevision());
        if (scaled != localized && compactOnTelevision()) {
            android.content.res.Configuration full = AppTheme.scaled(localized).getResources().getConfiguration();
            if (full.densityDpi != scaled.getResources().getConfiguration().densityDpi) fullSize = new android.content.res.Configuration(full);
        }
        super.attachBaseContext(scaled);
    }

    /** The configuration at the app's interface size while this screen is compact on a TV, else null. */
    private android.content.res.Configuration fullSize;

    /** Whether this screen lays out at {@link AppTheme#TV_COMPACT_PERCENT} on a TV: Settings' screens do. */
    protected boolean compactOnTelevision() {
        return false;
    }

    /** {@link #fullSizeContext}'s density, without making the context: the TV margin's (AdaptiveLayout). */
    public float fullSizeDensity() {
        return fullSize == null ? getResources().getDisplayMetrics().density
                : fullSize.densityDpi / (float) android.util.DisplayMetrics.DENSITY_DEFAULT;
    }

    /**
     * A context for views that keep the app's interface size on a screen that is compact on a TV: the shell bar, which
     * must not change size between its tabs. Without the compact size, the screen itself.
     */
    public android.content.Context fullSizeContext() {
        if (fullSize == null) return this;
        // The screen's theme, overlays included, on the full-size resources. Only a TV's screen is compact.
        android.view.ContextThemeWrapper wrapper = new android.view.ContextThemeWrapper(this, AppTheme.televisionTheme(this));
        wrapper.applyOverrideConfiguration(fullSize);
        AppTheme.applyOverlays(this, wrapper.getTheme());
        return wrapper;
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
        form = GlassForm.of(this);
    }

    /** The form this screen was laid out for. */
    private GlassForm form;

    /**
     * Rotating, split screen, folding or unfolding can move the window into another form: a screen that handles those
     * changes itself is laid out again for the new form, as one the system recreates is. Saved state keeps focus and scroll.
     */
    @Override
    public void onConfigurationChanged(android.content.res.Configuration newConfig) {
        super.onConfigurationChanged(newConfig);
        GlassForm now = GlassForm.of(this);
        if (form != null && now != form && !isFinishing() && !isDestroyed()) {
            form = now;
            recreate();
        }
    }

    @Override
    protected void onStart() {
        super.onStart();
        PreferenceManager.getDefaultSharedPreferences(this).registerOnSharedPreferenceChangeListener(appearanceListener);
    }

    @Override
    protected void onResume() {
        super.onResume();
        // Back on this screen: L1 and R1 may step again.
        shoulderStepping = false;
        refreshAppearance();
        hideBarsOnForcedTelevision();
        if (!(this instanceof PairLinkActivity)) LinkGate.showWaiting(this);
    }

    /** A TV has no system bars; the forced TV layout on a handheld fills the screen the same way. */
    private void hideBarsOnForcedTelevision() {
        if (AppTheme.forcedTelevision(this)) AppTheme.hideSystemBars(this);
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

    /** The shell bar's destination this screen belongs to, once {@link ShellBar#install} put the bar on it. */
    private ShellBar.Destination shellDestination;
    /** L1 or R1 has opened another destination and this screen has not come back since: a second press waits. */
    private boolean shoulderStepping;

    void setShellDestination(ShellBar.Destination destination) {
        shellDestination = destination;
        shoulderStepping = false;
    }

    /** A pad that reports its D-pad as keys and as a hat must not move the focus twice. */
    @Override
    public boolean dispatchKeyEvent(android.view.KeyEvent event) {
        return dpadEchoes.drop(event) || shoulderStep(event) || super.dispatchKeyEvent(event);
    }

    /**
     * L1 and R1 switch to the shell bar's destination left or right of this one, round at the ends. A dialog has
     * its own window, so its keys never come here; a text field keeps them for itself.
     */
    private boolean shoulderStep(android.view.KeyEvent event) {
        int code = event.getKeyCode();
        if (shellDestination == null || (code != android.view.KeyEvent.KEYCODE_BUTTON_L1 && code != android.view.KeyEvent.KEYCODE_BUTTON_R1)
                || getCurrentFocus() instanceof android.widget.EditText) {
            return false;
        }
        // A quick second press lands before the opened screen takes the keys; it would open the same tab again.
        if (event.getAction() == android.view.KeyEvent.ACTION_DOWN && event.getRepeatCount() == 0 && !shoulderStepping) {
            shoulderStepping = ShellBar.step(this, shellDestination, code == android.view.KeyEvent.KEYCODE_BUTTON_R1);
        }
        return true;
    }

    /** Releases that arrive while another window has focus never reach this one. */
    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        if (!hasFocus) dpadEchoes.reset();
        else hideBarsOnForcedTelevision();
        super.onWindowFocusChanged(hasFocus);
    }

    private void refreshAppearance() {
        if (!recreatingForTheme && !isFinishing() && !isDestroyed() &&
                !AppTheme.appearance(this).equals(appearance)) {
            recreatingForTheme = true;
            recreate();
        }
    }
}

package com.limelight.ui;

import static org.junit.Assert.assertEquals;

import android.app.Activity;
import android.app.Application;
import android.content.Context;
import android.os.Build;
import android.preference.PreferenceManager;
import android.view.View;
import android.widget.TextView;

import com.limelight.PcViewLifecycleTest;
import com.limelight.R;
import com.limelight.console.HomeSettings;
import com.limelight.preferences.GlPreferences;
import com.limelight.preferences.StreamSettings;
import com.limelight.ui.ShellBar.Destination;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

/**
 * nightjar#7 (Shield TV): Settings looked oversized on a TV next to Home and the library. On a TV Settings' screens lay
 * out at 85 % of the interface size, text, controls, spacing and dialogs alike; the shell bar and the TV's overscan
 * margin stay as large as on the other tabs, so switching tabs does not move them.
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni", qualifiers = "w960dp-h540dp-land-television-xhdpi")
public class TvSettingsScaleTest {
    private Context context;

    @Before
    public void setUp() {
        context = RuntimeEnvironment.getApplication();
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
        GlPreferences gl = GlPreferences.readPreferences(context);
        gl.savedFingerprint = Build.FINGERPRINT;
        gl.glRenderer = "Test renderer";
        gl.writePreferences();
        context.getSharedPreferences("FirstRun", 0).edit().putBoolean("pairing_guide_shown", true).commit();
    }

    @After
    public void tearDown() {
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
    }

    private float density() {
        return context.getResources().getDisplayMetrics().density;
    }

    @Test
    public void settingsOnATvAreEightyFivePercentAndThePcsStayFullSize() {
        try (var settings = Robolectric.buildActivity(StreamSettings.class).setup()) {
            assertEquals(density() * 0.85f, settings.get().getResources().getDisplayMetrics().density, 0.02f);
            assertEquals(Math.round(960 / 0.85f), settings.get().getResources().getConfiguration().screenWidthDp);
        }
        try (var pcs = Robolectric.buildActivity(PcViewLifecycleTest.Library.class, HomeSettings.pcList(context)).setup()) {
            assertEquals(density(), pcs.get().getResources().getDisplayMetrics().density, 0.02f);
        }
    }

    @Test
    public void theCompactSizeComesOnTopOfTheInterfaceSize() {
        PreferenceManager.getDefaultSharedPreferences(context).edit().putString(AppTheme.UI_SCALE, "115").commit();
        try (var settings = Robolectric.buildActivity(StreamSettings.class).setup()) {
            assertEquals(density() * 1.15f * 0.85f, settings.get().getResources().getDisplayMetrics().density, 0.02f);
        }
    }

    @Test
    public void aHandheldKeepsSettingsAtTheInterfaceSize() {
        PreferenceManager.getDefaultSharedPreferences(context).edit().putString(AppTheme.UI_MODE, AppTheme.UI_MODE_HANDHELD).commit();
        try (var settings = Robolectric.buildActivity(StreamSettings.class).setup()) {
            assertEquals(density(), settings.get().getResources().getDisplayMetrics().density, 0.02f);
        }
    }

    @Test
    public void theSettingsSubScreensAreCompactToo() {
        try (var profiles = Robolectric.buildActivity(com.limelight.preferences.StreamProfilesActivity.class).setup()) {
            assertEquals(density() * 0.85f, profiles.get().getResources().getDisplayMetrics().density, 0.02f);
        }
    }

    private static int[] shellMetrics(Activity screen) {
        View tab = screen.findViewById(R.id.shell_pcs);
        int[] at = new int[2];
        tab.getLocationInWindow(at);
        // The first tab starts on the screen's inset, the edge of the rows below it (ShellNavigationTest).
        int x = at[0] - screen.getResources().getDimensionPixelSize(R.dimen.shell_inset);
        return new int[] {tab.getHeight(), Math.round(((TextView) tab).getTextSize()), x, at[1]};
    }

    @Test
    public void theShellBarAndTheOverscanMarginStayAsOnThePcs() {
        int[] pcs;
        try (var screen = Robolectric.buildActivity(PcViewLifecycleTest.Library.class, HomeSettings.pcList(context))
                .setup().visible()) {
            pcs = shellMetrics(screen.get());
        }
        try (var screen = Robolectric.buildActivity(StreamSettings.class, ShellBar.intentFor(context, Destination.SETTINGS))
                .setup().visible()) {
            int[] settings = shellMetrics(screen.get());
            assertEquals("tab height", pcs[0], settings[0]);
            assertEquals("tab text size", pcs[1], settings[1]);
            assertEquals("the bar's edge: overscan and TV margin", pcs[2], settings[2]);
            assertEquals("tab y", pcs[3], settings[3]);
        }
    }
}

package com.limelight.ui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;
import android.content.res.Configuration;
import android.preference.PreferenceManager;

import com.limelight.PcViewLifecycleTest;
import com.limelight.R;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

/** Interface layout: TV or Handheld replaces the uiMode type, so every TV check and -television resource follows. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni", qualifiers = "w393dp-h851dp-night-xxhdpi")
public class UiModeTest {
    private Context context;

    @Before
    public void setUp() {
        context = RuntimeEnvironment.getApplication();
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
    }

    private void mode(String value) {
        PreferenceManager.getDefaultSharedPreferences(context).edit().putString(AppTheme.UI_MODE, value).commit();
    }

    private static int type(Context context) {
        return context.getResources().getConfiguration().uiMode & Configuration.UI_MODE_TYPE_MASK;
    }

    @Test
    public void parseAcceptsTvAndHandheldAndEverythingElseIsAutomatic() {
        assertEquals("tv", AppTheme.parseUiMode("tv"));
        assertEquals("handheld", AppTheme.parseUiMode("handheld"));
        for (String other : new String[] {null, "", "auto", "TV", "television", "phone"}) {
            assertEquals(other, "auto", AppTheme.parseUiMode(other));
        }
        assertEquals("auto", AppTheme.uiMode(context));
    }

    @Test
    public void automaticLeavesTheContextAlone() {
        assertTrue(AppTheme.scaled(context) == context);
        mode("auto");
        assertTrue(AppTheme.scaled(context) == context);
        mode("garbage");
        assertTrue(AppTheme.scaled(context) == context);
    }

    @Test
    public void tvReportsTheTelevisionTypeAndKeepsTheNightBits() {
        assertEquals(Configuration.UI_MODE_TYPE_NORMAL, type(context));
        int night = context.getResources().getConfiguration().uiMode & Configuration.UI_MODE_NIGHT_MASK;
        assertEquals(Configuration.UI_MODE_NIGHT_YES, night);
        mode("tv");
        Context tv = AppTheme.scaled(context);
        assertEquals(Configuration.UI_MODE_TYPE_TELEVISION, type(tv));
        assertEquals(night, tv.getResources().getConfiguration().uiMode & Configuration.UI_MODE_NIGHT_MASK);
        // The size is untouched when only the type is changed.
        assertEquals(context.getResources().getConfiguration().densityDpi, tv.getResources().getConfiguration().densityDpi);
        assertEquals(393, tv.getResources().getConfiguration().screenWidthDp);
    }

    @Test
    public void tvResolvesTelevisionResourcesAndAutomaticDoesNot() {
        assertFalse(context.getResources().getBoolean(R.bool.settings_two_pane));
        mode("tv");
        assertTrue(AppTheme.scaled(context).getResources().getBoolean(R.bool.settings_two_pane));
        mode("auto");
        assertFalse(AppTheme.scaled(context).getResources().getBoolean(R.bool.settings_two_pane));
    }

    @Test
    public void handheldOnATelevisionReportsTheNormalType() {
        Configuration television = new Configuration(context.getResources().getConfiguration());
        television.uiMode = (television.uiMode & ~Configuration.UI_MODE_TYPE_MASK) | Configuration.UI_MODE_TYPE_TELEVISION;
        Context onTv = context.createConfigurationContext(television);
        assertEquals(Configuration.UI_MODE_TYPE_TELEVISION, type(onTv));
        mode("handheld");
        Context handheld = AppTheme.scaled(onTv);
        assertEquals(Configuration.UI_MODE_TYPE_NORMAL, type(handheld));
        assertFalse(handheld.getResources().getBoolean(R.bool.settings_two_pane));
        mode("tv");
        assertTrue(AppTheme.scaled(onTv) == onTv);
    }

    @Test
    public void theTypeAndTheSizeApplyTogether() {
        mode("tv");
        PreferenceManager.getDefaultSharedPreferences(context).edit().putString(AppTheme.UI_SCALE, "150").commit();
        Configuration config = AppTheme.scaled(context).getResources().getConfiguration();
        assertEquals(Configuration.UI_MODE_TYPE_TELEVISION, config.uiMode & Configuration.UI_MODE_TYPE_MASK);
        assertEquals(Math.round(393 / 1.5f), config.screenWidthDp);
    }

    @Test
    public void anActivityTakesTheTypeThroughItsBaseContext() {
        mode("tv");
        try (var screen = Robolectric.buildActivity(MotionTest.Screen.class).setup()) {
            assertEquals(Configuration.UI_MODE_TYPE_TELEVISION, type(screen.get()));
        }
    }

    @Test
    public void changingTheSettingChangesTheAppearanceSoOpenScreensRecreate() {
        String before = AppTheme.appearance(context);
        mode("tv");
        assertFalse(before.equals(AppTheme.appearance(context)));
        assertTrue(AppTheme.affectsAppearance(AppTheme.UI_MODE));
        assertTrue(AppTheme.affectsAppearance(null));
    }

    public static class Counting extends NightjarActivity {
        static int created;
        static int lastType;

        @Override protected void onCreate(android.os.Bundle state) {
            super.onCreate(state);
            created++;
            lastType = type(this);
            setContentView(new android.widget.FrameLayout(this));
        }
    }

    @Test
    public void anOpenScreenRecreatesInTheNewLayoutWhenTheSettingChanges() {
        Counting.created = 0;
        try (var screen = Robolectric.buildActivity(Counting.class).setup()) {
            assertEquals(1, Counting.created);
            assertEquals(Configuration.UI_MODE_TYPE_NORMAL, Counting.lastType);
            mode("tv");
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertEquals(2, Counting.created);
            assertEquals(Configuration.UI_MODE_TYPE_TELEVISION, Counting.lastType);
        }
    }

    private static boolean barsHidden(android.app.Activity activity) {
        int flags = activity.getWindow().getDecorView().getSystemUiVisibility();
        return (flags & android.view.View.SYSTEM_UI_FLAG_HIDE_NAVIGATION) != 0 &&
                (flags & android.view.View.SYSTEM_UI_FLAG_FULLSCREEN) != 0;
    }

    @Test
    public void theForcedTvLayoutOnAPhoneHidesTheSystemBars() {
        mode("tv");
        assertTrue(AppTheme.forcedTelevision(context));
        try (var screen = Robolectric.buildActivity(MotionTest.Screen.class).setup()) {
            assertTrue(barsHidden(screen.get()));
            // Regaining focus hides them again after a swipe showed them.
            screen.get().getWindow().getDecorView().setSystemUiVisibility(0);
            assertFalse(barsHidden(screen.get()));
            screen.get().onWindowFocusChanged(true);
            assertTrue(barsHidden(screen.get()));
        }
    }

    @Test
    public void automaticAndHandheldKeepTheSystemBars() {
        for (String value : new String[] {"auto", "handheld"}) {
            mode(value);
            assertFalse(AppTheme.forcedTelevision(context));
            try (var screen = Robolectric.buildActivity(MotionTest.Screen.class).setup()) {
                assertFalse(value, barsHidden(screen.get()));
            }
        }
    }

    @Test
    public void aRealTelevisionKeepsItsOwnBehaviour() {
        org.robolectric.Shadows.shadowOf((android.app.UiModeManager) context.getSystemService(Context.UI_MODE_SERVICE))
                .currentModeType = Configuration.UI_MODE_TYPE_TELEVISION;
        mode("tv");
        assertFalse(AppTheme.forcedTelevision(context));
        try (var screen = Robolectric.buildActivity(MotionTest.Screen.class).setup()) {
            assertFalse(barsHidden(screen.get()));
        }
    }
}

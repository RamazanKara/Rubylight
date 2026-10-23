package com.limelight.ui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;
import android.content.res.Configuration;
import android.os.Build;
import android.preference.PreferenceManager;
import android.view.View;
import android.view.ViewGroup;
import android.widget.AbsListView;
import android.widget.HorizontalScrollView;

import com.limelight.PcViewLifecycleTest;
import com.limelight.console.HomeSettings;
import com.limelight.preferences.GlPreferences;
import com.limelight.preferences.StreamSettings;
import com.limelight.ui.ShellBar.Destination;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.List;

/** Interface size: the percentage scales density and the dp size follows; 150 % still fits a 393 dp phone. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni", qualifiers = "w393dp-h851dp-xxhdpi")
public class UiScaleTest {
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

    private void scale(String percent) {
        PreferenceManager.getDefaultSharedPreferences(context).edit().putString(AppTheme.UI_SCALE, percent).commit();
    }

    @Test
    public void parseAcceptsOnlyTheFiveSizesAndFallsBackToOneHundred() {
        for (int percent : AppTheme.UI_SCALES) assertEquals(percent, AppTheme.parseScale(String.valueOf(percent)));
        for (String bad : new String[] {null, "", "abc", "0", "-100", "99", "200", "1e2", "100.0"}) {
            assertEquals(bad, 100, AppTheme.parseScale(bad));
        }
        assertEquals(5, AppTheme.UI_SCALES.length);
        assertEquals(85, AppTheme.UI_SCALES[0]);
        assertEquals(150, AppTheme.UI_SCALES[4]);
    }

    @Test
    public void aStoredSizeReadsBackAndABadOneReadsAsOneHundred() {
        assertEquals(100, AppTheme.uiScalePercent(context));
        scale("130");
        assertEquals(130, AppTheme.uiScalePercent(context));
        scale("garbage");
        assertEquals(100, AppTheme.uiScalePercent(context));
    }

    @Test
    public void oneHundredPercentLeavesTheContextAlone() {
        assertTrue(AppTheme.scaled(context) == context);
    }

    @Test
    public void aScaledContextHasScaledDensityAndAMatchingSmallerDpSize() {
        Configuration base = context.getResources().getConfiguration();
        for (int percent : AppTheme.UI_SCALES) {
            scale(String.valueOf(percent));
            Context scaled = AppTheme.scaled(context);
            float factor = percent / 100f;
            Configuration config = scaled.getResources().getConfiguration();
            assertEquals(percent + " % density", Math.round(base.densityDpi * factor), config.densityDpi);
            assertEquals(percent + " % width", Math.round(base.screenWidthDp / factor), config.screenWidthDp);
            assertEquals(percent + " % height", Math.round(base.screenHeightDp / factor), config.screenHeightDp);
            assertEquals(percent + " % smallest", Math.round(base.smallestScreenWidthDp / factor), config.smallestScreenWidthDp);
        }
    }

    @Test
    public void anActivityTakesTheScaledDensityThroughItsAppCompatBase() {
        float density = context.getResources().getDisplayMetrics().density;
        scale("150");
        try (var screen = Robolectric.buildActivity(MotionTest.Screen.class).setup()) {
            assertEquals(density * 1.5f, screen.get().getResources().getDisplayMetrics().density, 0.02f);
            assertEquals(Math.round(393 / 1.5f), screen.get().getResources().getConfiguration().screenWidthDp);
        }
    }

    @Test
    public void theStreamActivityIsNotScaled() {
        float density = context.getResources().getDisplayMetrics().density;
        scale("150");
        com.limelight.Game game = Robolectric.buildActivity(com.limelight.Game.class).get();
        assertEquals(density, game.getResources().getDisplayMetrics().density, 0.02f);
    }

    private static int absoluteRight(View view) {
        int right = view.getWidth();
        for (View v = view; v.getParent() instanceof View; v = (View) v.getParent()) {
            View parent = (View) v.getParent();
            right += v.getLeft() - parent.getScrollX();
        }
        return right;
    }

    private static boolean inHorizontalScroller(View view) {
        for (View v = view; v.getParent() instanceof View; v = (View) v.getParent()) {
            if (v.getParent() instanceof HorizontalScrollView) return true;
        }
        return false;
    }

    private static void collectClickable(View view, List<View> out) {
        if (view.getVisibility() != View.VISIBLE) return;
        if ((view.isClickable() || view.isFocusable()) && view.getWidth() > 0 && !(view instanceof AbsListView)
                && !(view instanceof android.widget.ScrollView) && !(view instanceof HorizontalScrollView)
                && !(view instanceof androidx.core.widget.NestedScrollView)) out.add(view);
        if (view instanceof ViewGroup && !(view instanceof AbsListView)) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) collectClickable(group.getChildAt(i), out);
        }
    }

    private void assertFitsAndKeepsTouchTargets(String screen, android.app.Activity activity) {
        View decor = activity.getWindow().getDecorView();
        float density = activity.getResources().getDisplayMetrics().density;
        assertEquals(screen + " is laid out at 262 dp", 262, Math.round(decor.getWidth() / density));
        List<String> overflow = new ArrayList<>();
        List<String> small = new ArrayList<>();
        ArrayList<View> all = new ArrayList<>();
        collectAll(decor, all);
        for (View view : all) {
            if (!inHorizontalScroller(view) && absoluteRight(view) > decor.getWidth() + 1) {
                overflow.add(describe(view) + " ends at " + Math.round(absoluteRight(view) / density) + " dp");
            }
        }
        List<View> targets = new ArrayList<>();
        collectClickable(decor, targets);
        for (View view : targets) {
            if (view.getParent() instanceof AbsListView) continue;
            float minimum = 47.5f * density;
            if (view.getHeight() < minimum && view.getWidth() < minimum) {
                small.add(describe(view) + " " + Math.round(view.getWidth() / density) + "x" + Math.round(view.getHeight() / density) + " dp");
            }
        }
        assertTrue(screen + " overflows 262 dp: " + overflow, overflow.isEmpty());
        assertTrue(screen + " has touch targets under 48 dp: " + small, small.isEmpty());
        assertFalse(targets.isEmpty());
    }

    private static void collectAll(View view, List<View> out) {
        if (view.getVisibility() != View.VISIBLE) return;
        out.add(view);
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) collectAll(group.getChildAt(i), out);
        }
    }

    private static String describe(View view) {
        String name = view.getClass().getSimpleName();
        if (view.getId() != View.NO_ID) {
            try {
                name += "#" + view.getResources().getResourceEntryName(view.getId());
            } catch (RuntimeException ignored) {
                name += "#" + view.getId();
            }
        }
        return name;
    }

    @Test
    public void pcsAndSettingsAt150PercentFitA393DpPhoneWithFullSizeTouchTargets() {
        scale("150");
        try (var pcs = Robolectric.buildActivity(PcViewLifecycleTest.Library.class, HomeSettings.pcList(context))
                .setup().visible()) {
            assertFitsAndKeepsTouchTargets("PCs", pcs.get());
        }
        try (var settings = Robolectric.buildActivity(StreamSettings.class,
                ShellBar.intentFor(context, Destination.SETTINGS)).setup().visible()) {
            assertFitsAndKeepsTouchTargets("Settings", settings.get());
        }
    }
}

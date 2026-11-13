package com.limelight.ui;

import android.app.Activity;
import android.app.Application;
import android.app.Dialog;
import android.graphics.Color;
import android.preference.PreferenceManager;
import android.view.WindowManager;

import androidx.appcompat.app.AlertDialog;
import androidx.core.content.ContextCompat;
import androidx.core.graphics.ColorUtils;

import com.limelight.R;
import com.limelight.binding.input.keyboard.PcKeyboardController;
import com.limelight.binding.input.keyboard.PcKeyboardLayout;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;

import static org.junit.Assert.*;

/**
 * Everything over a stream wears the stream menu's glass: a dialog the menu's, without a dim, and what stays up while the
 * game plays (the statistics, the keyboards) the HUD's, see-through and never over a blur. A stream is its activity, in any
 * theme; a screen of the app's own keeps its page dialogs.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class, qualifiers = "w960dp-h540dp-land-xhdpi")
@GraphicsMode(GraphicsMode.Mode.NATIVE)
public class StreamGlassTest {
    /** A stream's activity, as Game is. */
    public static final class Stream extends Activity implements StreamWindow {
        @Override public boolean usesFullDisplayForVideo() {
            return false;
        }

        @Override public boolean isWindowedStream() {
            return false;
        }
    }

    /** A dialog whose window the system refuses, as when the stream finishes under it. */
    private static final class Refused extends Dialog {
        Refused(Activity owner) {
            super(owner);
            getWindow().setBackgroundDrawable(GlassPanelDrawable.menu(owner, false));
        }

        @Override public void show() {
            throw new WindowManager.BadTokenException("the stream is finishing");
        }
    }

    @After
    public void forget() {
        MenuBlurWindow.enabledForTest = null;
        highContrast(false);
    }

    private static void highContrast(boolean on) {
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit()
                .putBoolean(AppTheme.HIGH_CONTRAST, on).commit();
    }

    private static PcKeyboardController controller() {
        return new PcKeyboardController(PcKeyboardLayout.byId(PcKeyboardLayout.US), (vk, down, modifiers) -> { },
                new PcKeyboardController.Host() {
                    @Override public void close() { }
                    @Override public void changed() { }
                });
    }

    private static double ratio(int a, int b) {
        return ColorUtils.calculateContrast(a | 0xFF000000, b | 0xFF000000);
    }

    @Test
    public void aStreamIsItsActivityNotItsTheme() {
        try (var stream = Robolectric.buildActivity(Stream.class).setup();
             var home = Robolectric.buildActivity(Activity.class).setup()) {
            assertTrue(StreamGlass.overStream(stream.get()));
            // The debug showcase's home borrows the stream's theme: it still is a page.
            home.get().setTheme(R.style.StreamTheme);
            assertFalse(StreamGlass.overStream(home.get()));
        }
    }

    @Test
    public void aDialogOverAStreamWearsTheMenusGlassWithoutADimInEveryStreamTheme() {
        for (int theme : new int[] {R.style.StreamTheme, R.style.OledBlackTheme}) {
            try (var controller = Robolectric.buildActivity(Stream.class).setup().visible()) {
                Activity activity = controller.get();
                activity.setTheme(theme);
                MenuBlurWindow.enabledForTest = false;
                AlertDialog dialog = new NavigationDialogBuilder(activity).setTitle("Touch mode").setMessage("Trackpad").create();
                StreamGlass.show(dialog);
                GlassPanelDrawable glass = StreamGlass.glassOf(dialog.getWindow().getDecorView().getBackground());
                String name = activity.getResources().getResourceEntryName(theme);
                assertNotNull(name + ": the stream menu's glass", glass);
                assertEquals(name + ": no blur here, as opaque as the menu without one", 0xFA, Color.alpha(glass.fillColor()));
                assertEquals(name + ": the game stays undimmed around it", 0,
                        dialog.getWindow().getAttributes().flags & WindowManager.LayoutParams.FLAG_DIM_BEHIND);
                dialog.dismiss();
            }
        }
    }

    @Test
    public void aDialogOnAPageKeepsItsOwnGlassAndDim() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            AlertDialog dialog = new NavigationDialogBuilder(activity).setTitle("Pair").create();
            StreamGlass.show(dialog);
            assertNull(StreamGlass.glassOf(dialog.getWindow().getDecorView().getBackground()));
            assertNotEquals(0, dialog.getWindow().getAttributes().flags & WindowManager.LayoutParams.FLAG_DIM_BEHIND);
            dialog.dismiss();
        }
    }

    @Test
    public void aRefusedDialogTakesItsBlurWindowWithIt() {
        try (var controller = Robolectric.buildActivity(Stream.class).setup().visible()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            GlassTier.force(activity, GlassTier.LENS);
            MenuBlurWindow.enabledForTest = true;
            MenuBlurWindow[] opened = new MenuBlurWindow[1];
            try {
                StreamGlass.show(new Refused(activity), blur -> opened[0] = blur);
                fail("the refusal reaches the caller");
            } catch (WindowManager.BadTokenException expected) {
                // As before: the caller hears it.
            } finally {
                GlassTier.force(activity, null);
            }
            assertNotNull("a blur window opened for it", opened[0]);
            assertFalse("and closed with it", opened[0].isShowing());
        }
    }

    /**
     * nightjar#108: Android 14 and later keep a window where it was for its whole open animation, so the blur window, opened in
     * the corner before the panel has a size, showed there at the panel's size. It opens without an enter animation, keeps its
     * exit, and its blur rises once it lies under the panel.
     */
    @Test
    public void theBlurOpensWithoutAnAnimationAndRisesOnceUnderThePanel() {
        try (var controller = Robolectric.buildActivity(Stream.class).setup().visible()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            GlassTier.force(activity, GlassTier.LENS);
            MenuBlurWindow.enabledForTest = true;
            MenuBlurWindow blur;
            try {
                blur = MenuBlurWindow.openIfAllowed(activity, 40, on -> { });
            } finally {
                GlassTier.force(activity, null);
            }
            assertNotNull(blur);
            assertEquals(R.style.StreamMenuBlurAnimation, blur.windowAnimations());
            android.content.res.TypedArray animations = activity.obtainStyledAttributes(R.style.StreamMenuBlurAnimation,
                    new int[] {android.R.attr.windowEnterAnimation, android.R.attr.windowExitAnimation});
            try {
                assertEquals("no enter animation", 0, animations.getResourceId(0, 0));
                assertEquals("the panel's exit", R.anim.tv_sheet_exit, animations.getResourceId(1, 0));
            } finally {
                animations.recycle();
            }

            assertEquals("a pixel's blur in the corner", 1, blur.shownRadius());
            org.robolectric.shadows.ShadowLooper.idleMainLooper(400, java.util.concurrent.TimeUnit.MILLISECONDS);
            assertEquals("until it lies under the panel", 1, blur.shownRadius());
            blur.cover(300, 40, 600, 500);
            assertTrue("then it rises", blur.shownRadius() < 40);
            org.robolectric.shadows.ShadowLooper.idleMainLooper(400, java.util.concurrent.TimeUnit.MILLISECONDS);
            assertEquals("to the full radius with the panel's entrance", 40, blur.shownRadius());
            blur.cover(300, 60, 600, 460);
            assertEquals("a new page only moves it", 40, blur.shownRadius());
            blur.close();
            assertFalse(blur.isShowing());
        }
    }

    @Test
    public void recomposingStandsABlurredPixelForAMomentOnAnyTier() {
        try (var controller = Robolectric.buildActivity(Stream.class).setup().visible()) {
            Activity activity = controller.get();
            GlassTier.force(activity, GlassTier.SOLID);
            MenuBlurWindow.enabledForTest = true;
            MenuBlurWindow[] opened = new MenuBlurWindow[1];
            try {
                assertTrue(StreamGlass.recompose(activity, 120, blur -> opened[0] = blur));
            } finally {
                GlassTier.force(activity, null);
            }
            assertNotNull(opened[0]);
            assertTrue("it stands while the system composes", opened[0].isShowing());
            org.robolectric.shadows.ShadowLooper.idleMainLooper(120, java.util.concurrent.TimeUnit.MILLISECONDS);
            assertFalse("and goes after the moment", opened[0].isShowing());

            MenuBlurWindow.enabledForTest = false;
            assertFalse("not without the system's blur", StreamGlass.recompose(activity, 120));
        }
    }

    @Test
    public void theHudIsSeeThroughAndNeverOverABlur() {
        try (var controller = Robolectric.buildActivity(Stream.class).setup()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            GlassSpec hud = GlassSpec.hud(activity);
            assertEquals("a blur under it would compose every stream frame on the GPU", 0, hud.blur, 0);
            int fill = Color.alpha(GlassPanelDrawable.hud(activity, new float[8]).fillColor());
            assertTrue("lets the game through", fill < 0xE0);
            assertTrue("still a panel behind the text", fill >= 0xB0);
        }
    }

    @Test
    public void aKeyboardsOpacitySettingIsItsPanelsAlphaFromSeeThroughToOpaque() {
        try (var controller = Robolectric.buildActivity(Stream.class).setup()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            GlassPanelDrawable panel = GlassPanelDrawable.hud(activity, new float[8]);
            int recipe = panel.fillColor();
            panel.setFillOpacity(1f);
            assertEquals("100% is opaque", 0xFF, Color.alpha(panel.fillColor()));
            assertEquals("in the recipe's colour", recipe & 0xFFFFFF, panel.fillColor() & 0xFFFFFF);
            panel.setFillOpacity(0.5f);
            assertEquals(0x80, Color.alpha(panel.fillColor()));
            // The PC keyboard as the desktop controls: the panel takes the setting, the keys stay opaque.
            PcKeyboardOverlay overlay = new PcKeyboardOverlay(activity, "auto", 50, 50, (vk, down, modifiers) -> { }, () -> { });
            assertEquals(1f, overlay.getAlpha(), 0);
            assertEquals(0x80, Color.alpha(((GlassPanelDrawable) overlay.getBackground()).fillColor()));
        }
    }

    @Test
    public void highContrastKeysAndWellsAreSolidOverAStream() {
        highContrast(true);
        try (var controller = Robolectric.buildActivity(Stream.class).setup()) {
            Activity activity = controller.get();
            // The stream's activity never applies the app's overlays: what is drawn must not lean on them.
            activity.setTheme(R.style.StreamTheme);
            int solid = ContextCompat.getColor(activity, R.color.nj_glass_chrome_solid_fill);
            int rim = ContextCompat.getColor(activity, R.color.nj_hc_color_rim);
            for (GlassControlDrawable.Kind kind : new GlassControlDrawable.Kind[] {GlassControlDrawable.Kind.KEY, GlassControlDrawable.Kind.GROUP}) {
                GlassControlDrawable control = new GlassControlDrawable(activity, kind, 8);
                assertEquals(kind + " is solid", solid, control.restFill());
                assertEquals(kind + " has the light rim", rim, control.restRimTop());
            }
            assertEquals("the HUD is the menu's solid", 0xFF, Color.alpha(GlassPanelDrawable.hud(activity, new float[8]).fillColor()));
        }
    }

    @Test
    public void theFocusRingNeverVanishesIntoALockedKey() {
        for (boolean contrast : new boolean[] {false, true}) {
            highContrast(contrast);
            try (var controller = Robolectric.buildActivity(Stream.class).setup()) {
                Activity activity = controller.get();
                activity.setTheme(R.style.StreamTheme);
                PcKeyboardView view = new PcKeyboardView(activity, controller());
                int moon = ContextCompat.getColor(activity, contrast ? R.color.nj_hc_color_moon : R.color.nj_color_moon);
                int key = ContextCompat.getColor(activity, R.color.nj_color_surface_high);
                assertTrue("on a locked key, high contrast " + contrast, ratio(view.focusRing(true), moon) >= 3.0);
                assertTrue("on a key, high contrast " + contrast, ratio(view.focusRing(false), key) >= 3.0);
            }
        }
    }
}

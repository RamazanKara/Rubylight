package com.limelight.ui;

import android.app.Activity;
import android.app.Application;
import android.os.Build;
import android.preference.PreferenceManager;
import android.view.View;
import android.view.Window;
import android.view.WindowManager;

import androidx.appcompat.app.AlertDialog;

import com.limelight.R;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;
import org.robolectric.shadows.ShadowLooper;

import static org.junit.Assert.*;

/**
 * The page blurs behind a dialog and a sheet (Nightjar #61) where the device affords live glass and the system's switch is
 * on, under the dim it always had. Nothing over a stream ever asks for it.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class, qualifiers = "w960dp-h540dp-land-xhdpi")
@GraphicsMode(GraphicsMode.Mode.NATIVE)
public class PageBlurTest {
    @After
    public void forget() {
        MenuBlurWindow.enabledForTest = null;
        highContrast(false);
    }

    private static void highContrast(boolean on) {
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit()
                .putBoolean(AppTheme.HIGH_CONTRAST, on).commit();
    }

    private static Activity page(org.robolectric.android.controller.ActivityController<? extends Activity> controller) {
        Activity activity = controller.get();
        activity.setTheme(R.style.StreamTheme);
        return activity;
    }

    private static boolean blurred(Window window) {
        return (window.getAttributes().flags & WindowManager.LayoutParams.FLAG_BLUR_BEHIND) != 0;
    }

    private static boolean dimmed(Window window) {
        return (window.getAttributes().flags & WindowManager.LayoutParams.FLAG_DIM_BEHIND) != 0;
    }

    @Test
    public void theRuleNeedsAPageAndroid12LiveGlassAndTheSystemsSwitch() {
        for (GlassTier tier : GlassTier.values()) {
            boolean live = tier == GlassTier.LENS || tier == GlassTier.LIVE;
            assertEquals(tier.name(), live, PageBlur.allowed(false, Build.VERSION_CODES.S, tier, true));
            assertFalse(tier + " over a stream", PageBlur.allowed(true, Build.VERSION_CODES.UPSIDE_DOWN_CAKE, tier, true));
            assertFalse(tier + " on Android 11", PageBlur.allowed(false, Build.VERSION_CODES.R, tier, true));
            assertFalse(tier + " with the system's switch off", PageBlur.allowed(false, Build.VERSION_CODES.S, tier, false));
        }
    }

    @Test
    public void aDialogOnAPageBlursThePageUnderItsDim() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = page(controller);
            MenuBlurWindow.enabledForTest = true;
            AlertDialog dialog = new NavigationDialogBuilder(activity).setTitle("Pair").create();
            dialog.show();
            ShadowLooper.idleMainLooper();
            Window window = dialog.getWindow();
            assertTrue("the page blurs", blurred(window));
            assertTrue("a radius the system can see", window.getAttributes().getBlurBehindRadius() > 0);
            assertTrue("the dim stays", dimmed(window));
            assertEquals(GlassPanelDrawable.dialogBackdropDim(activity), window.getAttributes().dimAmount, 0.001f);
            dialog.dismiss();
        }
    }

    @Test
    public void whereTheBlurIsNotAllowedTheDimAloneStays() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = page(controller);
            // The system's switch is off: battery saver, a device without the effect, or the user's own choice.
            MenuBlurWindow.enabledForTest = false;
            AlertDialog off = new NavigationDialogBuilder(activity).setTitle("Pair").create();
            off.show();
            assertFalse(blurred(off.getWindow()));
            assertEquals(0, off.getWindow().getAttributes().getBlurBehindRadius());
            assertTrue(dimmed(off.getWindow()));
            off.dismiss();

            // High contrast lands on the solid tier: no live glass of any kind.
            MenuBlurWindow.enabledForTest = true;
            highContrast(true);
            AlertDialog solid = new NavigationDialogBuilder(activity).setTitle("Pair").create();
            solid.show();
            assertFalse(blurred(solid.getWindow()));
            assertTrue(dimmed(solid.getWindow()));
            solid.dismiss();
        }
    }

    @Test
    public void nothingOverAStreamEverAsksForThePagesBlur() {
        try (var controller = Robolectric.buildActivity(StreamGlassTest.Stream.class).setup().visible()) {
            Activity stream = controller.get();
            stream.setTheme(R.style.StreamTheme);
            // Even on a device that could: the stream needs the GPU time.
            MenuBlurWindow.enabledForTest = true;

            AlertDialog dialog = new NavigationDialogBuilder(stream).setTitle("Touch mode").create();
            assertNull(PageBlur.behind(dialog));
            StreamGlass.show(dialog);
            assertFalse("a dialog over the stream", blurred(dialog.getWindow()));
            dialog.dismiss();

            ActionSheet sheet = new ActionSheet(stream, "Display");
            sheet.show();
            assertNull(sheet.pageBlur());
            assertFalse("a sheet over the stream", blurred(sheet.getWindow()));
            sheet.dismiss();

            ActionSheet menu = new ActionSheet(stream, "Skybound", true);
            menu.show();
            assertNull(menu.pageBlur());
            assertFalse("the stream menu", blurred(menu.getWindow()));
            menu.dismiss();

            View anchor = new View(stream);
            stream.setContentView(anchor);
            ActionSheet hung = ActionSheet.under(stream, "PC", anchor);
            hung.show();
            assertNull(hung.pageBlur());
            assertFalse("a panel hung under a button of the stream", blurred(hung.getWindow()));
            hung.dismiss();
        }
    }

    @Test
    public void aSheetAndAPanelOnAPageBlurItAndFollowTheSwitchWhenShownAgain() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = page(controller);
            MenuBlurWindow.enabledForTest = true;
            ActionSheet sheet = new ActionSheet(activity, "Options");
            sheet.show();
            ShadowLooper.idleMainLooper();
            assertTrue("a sheet", blurred(sheet.getWindow()));
            assertTrue(dimmed(sheet.getWindow()));
            sheet.dismiss();

            // Battery saver came on in between: the same sheet, shown again, is the dim alone.
            MenuBlurWindow.enabledForTest = false;
            sheet.show();
            assertFalse(blurred(sheet.getWindow()));
            assertEquals(0, sheet.getWindow().getAttributes().getBlurBehindRadius());
            assertTrue(dimmed(sheet.getWindow()));
            sheet.dismiss();

            MenuBlurWindow.enabledForTest = true;
            View anchor = new View(activity);
            activity.setContentView(anchor);
            ActionSheet hung = ActionSheet.under(activity, "PC", anchor);
            hung.show();
            assertTrue("a panel hung under Home's capsule", blurred(hung.getWindow()));
            assertTrue(dimmed(hung.getWindow()));
            hung.dismiss();
        }
    }

    @Test
    public void aPanelWithoutAnAnchorIsTheStreamMenusAndLeavesThePageAlone() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = page(controller);
            MenuBlurWindow.enabledForTest = true;
            // The debug showcase shows the stream menu on a page: it keeps its own glass, with no dim and no whole-page blur.
            ActionSheet menu = new ActionSheet(activity, "Skybound", true);
            menu.show();
            assertNull(menu.pageBlur());
            assertFalse(blurred(menu.getWindow()));
            assertFalse(dimmed(menu.getWindow()));
            menu.dismiss();
        }
    }
}

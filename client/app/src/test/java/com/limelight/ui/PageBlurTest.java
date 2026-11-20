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
import static org.robolectric.Shadows.shadowOf;

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
        PageBlur.blursOffForTest = null;
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
    public void withoutTheSystemsBlurTheAppBlursItsOwnPageUnlessTheUserTurnedBlursOff() {
        int api = Build.VERSION_CODES.UPSIDE_DOWN_CAKE;
        for (GlassTier tier : GlassTier.values()) {
            boolean live = tier == GlassTier.LENS || tier == GlassTier.LIVE;
            assertEquals(tier.name(), live ? PageBlur.Kind.WINDOW : PageBlur.Kind.NONE, PageBlur.kind(false, api, tier, true, false));
            // A device that ships Android 12+ without cross-window blur (an iQOO 7 on Android 14, #61): the app blurs the page.
            assertEquals(tier.name(), live ? PageBlur.Kind.PAGE : PageBlur.Kind.NONE, PageBlur.kind(false, api, tier, false, false));
            assertEquals(tier + " turned off by the user", PageBlur.Kind.NONE, PageBlur.kind(false, api, tier, false, true));
            assertEquals(tier + " over a stream", PageBlur.Kind.NONE, PageBlur.kind(true, api, tier, false, false));
            assertEquals(tier + " over a stream", PageBlur.Kind.NONE, PageBlur.kind(true, api, tier, true, false));
            assertEquals(tier + " on Android 11", PageBlur.Kind.NONE, PageBlur.kind(false, Build.VERSION_CODES.R, tier, false, false));
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
            // The system's switch is off and the user turned window blurs off: no blur of either kind.
            MenuBlurWindow.enabledForTest = false;
            PageBlur.blursOffForTest = true;
            AlertDialog off = new NavigationDialogBuilder(activity).setTitle("Pair").create();
            off.show();
            ShadowLooper.idleMainLooper();
            assertFalse(blurred(off.getWindow()));
            assertEquals(0, off.getWindow().getAttributes().getBlurBehindRadius());
            assertFalse(PageBlur.pageBlurred(activity));
            assertTrue(dimmed(off.getWindow()));
            off.dismiss();
            PageBlur.blursOffForTest = false;

            // High contrast lands on the solid tier: no live glass of any kind.
            MenuBlurWindow.enabledForTest = true;
            highContrast(true);
            AlertDialog solid = new NavigationDialogBuilder(activity).setTitle("Pair").create();
            solid.show();
            assertFalse(blurred(solid.getWindow()));
            assertTrue(dimmed(solid.getWindow()));
            solid.dismiss();

            // With the system's switch off as well, the app's own blur stays off too.
            MenuBlurWindow.enabledForTest = false;
            AlertDialog contrast = new NavigationDialogBuilder(activity).setTitle("Pair").create();
            contrast.show();
            ShadowLooper.idleMainLooper();
            assertFalse(PageBlur.pageBlurred(activity));
            contrast.dismiss();
        }
    }

    @Test
    public void withoutTheSystemsBlurTheAppBlursThePageWhileTheDialogIsUp() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = page(controller);
            // A device on Android 12+ whose system has no cross-window blur.
            MenuBlurWindow.enabledForTest = false;
            PageBlur.blursOffForTest = false;
            AlertDialog dialog = new NavigationDialogBuilder(activity).setTitle("Pair").create();
            assertFalse("nothing before it shows", PageBlur.pageBlurred(activity));
            dialog.show();
            ShadowLooper.idleMainLooper();
            assertFalse("no system blur asked for", blurred(dialog.getWindow()));
            assertTrue("the page blurs in the app", PageBlur.pageBlurred(activity));
            assertTrue("the dim stays", dimmed(dialog.getWindow()));

            // A sheet over the same page, and the dialog closing first: the page stays blurred under the sheet.
            ActionSheet sheet = new ActionSheet(activity, "Options");
            sheet.show();
            ShadowLooper.idleMainLooper();
            assertEquals(PageBlur.Kind.PAGE, sheet.pageBlur().kind());
            dialog.dismiss();
            ShadowLooper.idleMainLooper();
            assertTrue("held by the sheet", PageBlur.pageBlurred(activity));
            sheet.dismiss();
            ShadowLooper.idleMainLooper();
            assertFalse("gone with the last window", PageBlur.pageBlurred(activity));
        }
    }

    /** A row's press feedback: counts how often it is told to end, and redraws its row then, as a ripple does. */
    private static final class Press extends android.graphics.drawable.ColorDrawable {
        int jumps;

        @Override
        public void jumpToCurrentState() {
            jumps++;
            invalidateSelf();
        }
    }

    /** Draws the page as a screen does: only a drawn view passes an invalidation on to its window. */
    private static void draw(View page) {
        page.draw(new android.graphics.Canvas(android.graphics.Bitmap.createBitmap(
                page.getWidth(), page.getHeight(), android.graphics.Bitmap.Config.ARGB_8888)));
    }

    /** The page's window loses its focus, as to a dialog: what its window tells its tree observer (hidden API). */
    private static void loseFocus(View page) {
        try {
            java.lang.reflect.Method dispatch = android.view.ViewTreeObserver.class
                    .getDeclaredMethod("dispatchOnWindowFocusChange", boolean.class);
            dispatch.setAccessible(true);
            dispatch.invoke(page.getViewTreeObserver(), false);
        } catch (ReflectiveOperationException e) {
            throw new AssertionError(e);
        }
    }

    /** One frame of the page's window: true when it asked to redraw the page whole. */
    private static boolean frame(View page) {
        shadowOf(page).clearWasInvalidated();
        page.getViewTreeObserver().dispatchOnPreDraw();
        boolean whole = shadowOf(page).wasInvalidated();
        draw(page);
        return whole;
    }

    @Test
    public void aRowRepaintedUnderTheDialogRedrawsTheWholeBlurredPage() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = page(controller);
            Press press = new Press();
            View row = new View(activity);
            row.setBackground(press);
            activity.setContentView(row);
            ShadowLooper.idleMainLooper();
            View page = activity.getWindow().getDecorView();
            draw(page);
            row.setPressed(true);
            // A phone on Android 12+ without the system's window blur: the app blurs the page (Settings, #61).
            MenuBlurWindow.enabledForTest = false;
            PageBlur.blursOffForTest = false;
            AlertDialog dialog = new NavigationDialogBuilder(activity).setTitle("Stream smoothness").create();
            int beforeBlur = press.jumps;
            dialog.show();
            ShadowLooper.idleMainLooper();
            assertTrue(PageBlur.pageBlurred(activity));
            assertTrue("the tapped row's press ends as the blur starts, instead of fading under it", press.jumps > beforeBlur);
            assertTrue("the press that ended redraws the whole page", frame(page));
            for (int i = 0; i < 5; i++) assertFalse("then a still page is left alone, frame " + i, frame(page));

            // A pressed view lets go as the page's window loses its focus to the dialog: its press ends at once.
            int jumps = press.jumps;
            row.setPressed(false);
            loseFocus(page);
            assertTrue(press.jumps > jumps);
            assertTrue(frame(page));
            for (int i = 0; i < 5; i++) assertFalse("no redraw feeds the next, frame " + i, frame(page));

            // Something that keeps changing (a spinner behind the dialog) redraws the page whole on each of its frames,
            // and its drawables are not ended over and over.
            jumps = press.jumps;
            for (int i = 0; i < 5; i++) {
                row.invalidate();
                assertTrue("the whole page redraws, so its blur has no edge at the row, frame " + i, frame(page));
            }
            assertEquals(jumps, press.jumps);
            assertFalse(frame(page));

            // Without the blur, a row redraws on its own again, and the window's focus ends nothing.
            dialog.dismiss();
            ShadowLooper.idleMainLooper();
            assertFalse(PageBlur.pageBlurred(activity));
            draw(page);
            jumps = press.jumps;
            loseFocus(page);
            assertEquals(jumps, press.jumps);
            row.invalidate();
            assertFalse(frame(page));
        }
    }

    @Test
    public void whereTheSystemBlursThePageItRedrawsAsEver() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = page(controller);
            View row = new View(activity);
            activity.setContentView(row);
            ShadowLooper.idleMainLooper();
            View page = activity.getWindow().getDecorView();
            // The system composes its blur from the whole screen on every frame: nothing to add.
            MenuBlurWindow.enabledForTest = true;
            AlertDialog dialog = new NavigationDialogBuilder(activity).setTitle("Stream smoothness").create();
            dialog.show();
            ShadowLooper.idleMainLooper();
            assertTrue(blurred(dialog.getWindow()));
            draw(page);
            shadowOf(page).clearWasInvalidated();
            row.invalidate();
            page.getViewTreeObserver().dispatchOnPreDraw();
            assertFalse(shadowOf(page).wasInvalidated());
            dialog.dismiss();
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

            // Nor does the app blur the stream itself where the system has no blur.
            MenuBlurWindow.enabledForTest = false;
            PageBlur.blursOffForTest = false;
            AlertDialog noSystemBlur = new NavigationDialogBuilder(stream).setTitle("Touch mode").create();
            StreamGlass.show(noSystemBlur);
            ShadowLooper.idleMainLooper();
            assertFalse(PageBlur.pageBlurred(stream));
            noSystemBlur.dismiss();
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

            // The system's switch went off in between: the same sheet, shown again, has the app blur the page instead.
            MenuBlurWindow.enabledForTest = false;
            PageBlur.blursOffForTest = false;
            sheet.show();
            ShadowLooper.idleMainLooper();
            assertFalse(blurred(sheet.getWindow()));
            assertEquals(0, sheet.getWindow().getAttributes().getBlurBehindRadius());
            assertTrue(PageBlur.pageBlurred(activity));
            assertTrue(dimmed(sheet.getWindow()));
            sheet.dismiss();
            ShadowLooper.idleMainLooper();
            assertFalse(PageBlur.pageBlurred(activity));

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

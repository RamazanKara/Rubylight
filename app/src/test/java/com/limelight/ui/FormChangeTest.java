package com.limelight.ui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.os.Bundle;

import com.limelight.R;
import com.limelight.pin.PinLockActivity;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.util.ReflectionHelpers;

/**
 * A shell page whose chrome follows the form is laid out again when the window changes form, and only then. Every other
 * screen survives the change with what it was doing: a rotation recreated nearly every screen and cancelled its work.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class, qualifiers = "w411dp-h891dp-port")
public class FormChangeTest {
    public static class Screen extends NightjarActivity {
        static int created;

        @Override protected void onCreate(Bundle state) {
            setTheme(R.style.AppTheme);
            super.onCreate(state);
            created++;
        }
    }

    /** A shell page: its chrome follows the form, unless its work holds it. */
    public static class ShellPage extends Screen {
        static boolean held;

        @Override protected boolean followsForm() {
            return true;
        }

        @Override protected boolean holdsForm() {
            return held;
        }
    }

    private static void toHandheld(NightjarActivity screen) {
        RuntimeEnvironment.setQualifiers("w900dp-h700dp-land");
        screen.onConfigurationChanged(screen.getResources().getConfiguration());
        ShadowLooper.idleMainLooper();
        assertEquals(GlassForm.HANDHELD, GlassForm.of(screen));
    }

    @Test
    public void unfoldingIntoTheHandheldFormLaysAShellPageOutAgain() {
        Screen.created = 0;
        ShellPage.held = false;
        try (var controller = Robolectric.buildActivity(ShellPage.class).setup()) {
            ShellPage screen = controller.get();
            assertEquals(GlassForm.PHONE, GlassForm.of(screen));

            // A taller phone window in landscape, still under 600 dp wide: the same form.
            RuntimeEnvironment.setQualifiers("w560dp-h411dp-land");
            screen.onConfigurationChanged(screen.getResources().getConfiguration());
            ShadowLooper.idleMainLooper();
            assertEquals("no new form, nothing laid out again", 1, Screen.created);

            toHandheld(screen);
            assertEquals("the handheld's layout, not the phone's under it", 2, Screen.created);
            assertTrue("a subclass is told, so it does not recreate a second time", screen.recreatingForForm());
        }
    }

    @Test
    public void aScreenThatDoesNotFollowTheFormSurvivesTheChange() {
        Screen.created = 0;
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            Screen screen = controller.get();
            toHandheld(screen);
            assertEquals(1, Screen.created);
            assertFalse(screen.recreatingForForm());
            assertFalse(screen.isDestroyed());
        }
    }

    @Test
    public void workThatHoldsTheFormKeepsThePageUntilItIsDoneThenThePageFollows() {
        Screen.created = 0;
        ShellPage.held = true;
        try (var controller = Robolectric.buildActivity(ShellPage.class).setup()) {
            ShellPage screen = controller.get();
            toHandheld(screen);
            assertEquals("pairing, a wake or an add still runs on the page", 1, Screen.created);
            screen.onWindowFocusChanged(true);
            ShadowLooper.idleMainLooper();
            assertEquals("still held", 1, Screen.created);

            ShellPage.held = false;
            screen.onWindowFocusChanged(true);
            ShadowLooper.idleMainLooper();
            assertEquals("done: the page follows the form", 2, Screen.created);
        } finally {
            ShellPage.held = false;
        }
    }

    /** A resume found a new theme and a new form at once: one recreate covers both, not two in a row. */
    @Test
    public void aNewThemeAndANewFormAtOnceLayThePageOutOnce() {
        Screen.created = 0;
        ShellPage.held = true;
        try (var controller = Robolectric.buildActivity(ShellPage.class).setup()) {
            ShellPage screen = controller.get();
            toHandheld(screen);
            ShellPage.held = false;
            ReflectionHelpers.setField(screen, "appearance", "another theme");
            controller.pause().resume();
            assertTrue("the theme lays it out again", (boolean) ReflectionHelpers.getField(screen, "recreatingForTheme"));
            assertFalse("the form waits for that", screen.recreatingForForm());
        } finally {
            ShellPage.held = false;
        }
    }

    /**
     * Work that ends and is followed at once by more (a wake moved to another PC) must not lay the page out between the two:
     * a page that follows soon asks once the call running now is done.
     */
    @Test
    public void followingSoonWaitsForWorkThatStartsRightAfter() {
        Screen.created = 0;
        ShellPage.held = true;
        try (var controller = Robolectric.buildActivity(ShellPage.class).setup()) {
            ShellPage screen = controller.get();
            toHandheld(screen);
            ShellPage.held = false;
            ReflectionHelpers.callInstanceMethod(screen, "followFormSoon");
            ShellPage.held = true;
            ShadowLooper.idleMainLooper();
            assertEquals("the next work holds it", 1, Screen.created);
            assertFalse(screen.recreatingForForm());
        } finally {
            ShellPage.held = false;
        }
    }

    @Test
    @Config(shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void choosingAPinKeepsItsFirstEntryThroughAFormChange() {
        try (var controller = Robolectric.buildActivity(PinLockActivity.class,
                PinLockActivity.forSet(RuntimeEnvironment.getApplication())).setup()) {
            PinLockActivity screen = controller.get();
            byte[] entry = ReflectionHelpers.getField(screen, "entry");
            for (int i = 0; i < 4; i++) entry[i] = (byte) ('1' + i);
            ReflectionHelpers.setField(screen, "count", 4);
            ReflectionHelpers.callInstanceMethod(screen, "submit");
            assertNotNull("the first entry waits for its confirmation", ReflectionHelpers.getField(screen, "first"));

            toHandheld(screen);
            assertSame(screen, controller.get());
            assertFalse("not laid out again", screen.isDestroyed());
            assertNotNull("the first entry survives", ReflectionHelpers.getField(screen, "first"));
        }
    }
}

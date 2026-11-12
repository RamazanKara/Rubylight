package com.limelight.ui;

import android.app.Application;
import android.view.KeyEvent;

import androidx.appcompat.app.AlertDialog;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;

import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class NavigationDialogBuilderTest {
    @Test public void cancellingGameReplacementOrPcDeletionRunsTheDeclineCallback() {
        ActivityController<UiNavigationTest.Screen> activity = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup();
        AtomicInteger accepted = new AtomicInteger(), declined = new AtomicInteger();
        try {
            com.limelight.utils.UiHelper.displayQuitConfirmationDialog(activity.get(), accepted::incrementAndGet, declined::incrementAndGet);
            AlertDialog replacement = (AlertDialog) org.robolectric.shadows.ShadowDialog.getLatestDialog();
            press(replacement, KeyEvent.KEYCODE_BUTTON_B);
            assertFalse(replacement.isShowing());
            assertEquals(0, accepted.get());
            assertEquals(1, declined.get());
            com.limelight.nvstream.http.ComputerDetails computer = new com.limelight.nvstream.http.ComputerDetails();
            computer.name = "PC";
            com.limelight.utils.UiHelper.displayDeletePcConfirmationDialog(activity.get(), computer,
                    accepted::incrementAndGet, declined::incrementAndGet);
            AlertDialog deletion = (AlertDialog) org.robolectric.shadows.ShadowDialog.getLatestDialog();
            press(deletion, KeyEvent.KEYCODE_BUTTON_B);
            assertFalse(deletion.isShowing());
            assertEquals(0, accepted.get());
            assertEquals(2, declined.get());
        } finally { activity.destroy(); }
    }

    private static void press(AlertDialog dialog, int key) {
        dialog.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_DOWN, key));
        dialog.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_UP, key));
        Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
    }

    @Test public void bAndEscapeCancelWithoutAcceptingAConfirmation() {
        ActivityController<UiNavigationTest.Screen> activity = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup();
        AtomicInteger accepted = new AtomicInteger(), cancelled = new AtomicInteger();
        try {
            for (int key : new int[] {KeyEvent.KEYCODE_BUTTON_B, KeyEvent.KEYCODE_ESCAPE}) {
                AlertDialog dialog = new NavigationDialogBuilder(activity.get()).setTitle("Confirm")
                        .setPositiveButton(android.R.string.ok, (target, which) -> accepted.incrementAndGet())
                        .setNegativeButton(android.R.string.cancel, null)
                        .setOnCancelListener(target -> cancelled.incrementAndGet()).show();
                press(dialog, key);
                assertFalse("Dialog still open after key " + key, dialog.isShowing());
            }
            assertEquals(0, accepted.get());
            assertEquals(2, cancelled.get());
        } finally { activity.destroy(); }
    }

    @Test public void nestedPickerBackReturnsToItsParentWithoutChoosingAnything() {
        ActivityController<UiNavigationTest.Screen> activity = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup();
        AtomicInteger chosen = new AtomicInteger();
        AlertDialog parent = new NavigationDialogBuilder(activity.get()).setTitle("Settings").show();
        try {
            AlertDialog picker = new NavigationDialogBuilder(activity.get())
                    .setSingleChoiceItems(new String[] {"One", "Two"}, 0,
                            (dialog, which) -> chosen.incrementAndGet()).show();
            press(picker, KeyEvent.KEYCODE_BUTTON_B);
            assertFalse(picker.isShowing());
            assertTrue(parent.isShowing());
            assertEquals(0, chosen.get());
            press(parent, KeyEvent.KEYCODE_BUTTON_B);
            assertFalse(parent.isShowing());
        } finally { parent.dismiss(); activity.destroy(); }
    }

    // Nightjar #81: a dialog has no Y hint. Y must count as taken, or Android sends it again as Space on the focused button.
    @Test public void aPadsYNeverPressesTheFocusedButton() {
        ActivityController<UiNavigationTest.Screen> activity = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup();
        AtomicInteger accepted = new AtomicInteger(), custom = new AtomicInteger();
        AlertDialog dialog = new NavigationDialogBuilder(activity.get()).setTitle("Confirm")
                .setPositiveButton(android.R.string.ok, (target, which) -> accepted.incrementAndGet()).show();
        AlertDialog own = new NavigationDialogBuilder(activity.get()).setTitle("Capture")
                .setOnKeyListener((target, key, event) -> {
                    if (key != KeyEvent.KEYCODE_BUTTON_Y) return false;
                    if (event.getAction() == KeyEvent.ACTION_UP) custom.incrementAndGet();
                    return true;
                }).show();
        try {
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).requestFocus();
            for (int action : new int[] {KeyEvent.ACTION_DOWN, KeyEvent.ACTION_UP}) {
                assertTrue(dialog.dispatchKeyEvent(new KeyEvent(action, KeyEvent.KEYCODE_BUTTON_Y)));
            }
            Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
            assertTrue(dialog.isShowing());
            assertEquals(0, accepted.get());
            // A dialog that reads Y itself still gets it first.
            press(own, KeyEvent.KEYCODE_BUTTON_Y);
            assertEquals(1, custom.get());
            assertTrue(own.isShowing());
        } finally { dialog.dismiss(); own.dismiss(); activity.destroy(); }
    }

    // A dialog's key listener runs before its views: with a text field focused, Y must reach the field, whose
    // Space types a space, while the buttons beside it stay unpressed.
    @Test public void aPadsYInADialogsTextFieldReachesTheField() {
        ActivityController<UiNavigationTest.Screen> activity = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup();
        AtomicInteger accepted = new AtomicInteger(), typed = new AtomicInteger();
        android.widget.EditText field = new android.widget.EditText(activity.get());
        field.setOnKeyListener((view, key, event) -> {
            if (key != KeyEvent.KEYCODE_BUTTON_Y) return false;
            if (event.getAction() == KeyEvent.ACTION_UP) typed.incrementAndGet();
            return true;
        });
        AlertDialog dialog = new NavigationDialogBuilder(activity.get()).setTitle("Name").setView(field)
                .setPositiveButton(android.R.string.ok, (target, which) -> accepted.incrementAndGet()).show();
        try {
            // Laid out, so keys travel down to the focused view.
            Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
            assertTrue(field.requestFocus());
            assertSame(field, dialog.getCurrentFocus());
            press(dialog, KeyEvent.KEYCODE_BUTTON_Y);
            assertEquals("the field got Y", 1, typed.get());
            // On the button Y is taken again and presses nothing.
            assertTrue(dialog.getButton(AlertDialog.BUTTON_POSITIVE).requestFocus());
            press(dialog, KeyEvent.KEYCODE_BUTTON_Y);
            Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
            assertEquals(1, typed.get());
            assertEquals(0, accepted.get());
            assertTrue(dialog.isShowing());
        } finally { dialog.dismiss(); activity.destroy(); }
    }

    @Test public void cancelledKeysCustomHandlersAndNoncancelableDialogsRetainTheirBehavior() {
        ActivityController<UiNavigationTest.Screen> activity = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup();
        AlertDialog locked = new NavigationDialogBuilder(activity.get()).setCancelable(false).show();
        AlertDialog custom = new NavigationDialogBuilder(activity.get())
                .setOnKeyListener((dialog, key, event) -> key == KeyEvent.KEYCODE_BUTTON_B).show();
        try {
            press(locked, KeyEvent.KEYCODE_BUTTON_B);
            assertTrue(locked.isShowing());
            press(custom, KeyEvent.KEYCODE_BUTTON_B);
            assertTrue(custom.isShowing());
            custom.dispatchKeyEvent(KeyEvent.changeFlags(new KeyEvent(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_ESCAPE),
                    KeyEvent.FLAG_CANCELED));
            assertTrue(custom.isShowing());
            press(custom, KeyEvent.KEYCODE_ESCAPE);
            assertFalse(custom.isShowing());
        } finally { locked.dismiss(); custom.dismiss(); activity.destroy(); }
    }

    /** A pairing PIN stands on the sky, not over the page's heading; the page comes back, for a screen reader too, when it goes. */
    @Test public void aDialogOnItsOwnSurfaceHidesThePageUnderTheSkyWhileItShows() {
        ActivityController<UiNavigationTest.Screen> activity = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup().visible();
        try {
            android.view.ViewGroup content = activity.get().findViewById(android.R.id.content);
            android.view.View page = content.getChildAt(0);
            assertEquals(1, content.getChildCount());
            AlertDialog dialog = new NavigationDialogBuilder(activity.get()).ownSurface().setTitle("Pair with PC")
                    .setMessage("1234").setNegativeButton(android.R.string.cancel, null).create();
            dialog.show();
            frame();
            assertEquals("the sky covers the page", 2, content.getChildCount());
            assertTrue(content.getChildAt(1) instanceof NightSkyView);
            assertEquals(android.view.View.IMPORTANT_FOR_ACCESSIBILITY_NO_HIDE_DESCENDANTS, page.getImportantForAccessibility());
            assertEquals("no dim over the sky", 0, dialog.getWindow().getAttributes().flags
                    & android.view.WindowManager.LayoutParams.FLAG_DIM_BEHIND);

            dialog.getButton(AlertDialog.BUTTON_NEGATIVE).performClick();
            frame();
            assertFalse(dialog.isShowing());
            assertEquals("the page is back", 1, content.getChildCount());
            assertEquals(android.view.View.IMPORTANT_FOR_ACCESSIBILITY_AUTO, page.getImportantForAccessibility());

            // Any other dialog stays over its dimmed page.
            AlertDialog plain = new NavigationDialogBuilder(activity.get()).setTitle("Remove PC").show();
            frame();
            assertEquals(1, content.getChildCount());
            plain.dismiss();
        } finally { activity.destroy(); }
    }

    /** A window's first traversal attaches its views; it waits for the next frame. */
    private static void frame() {
        Shadows.shadowOf(android.os.Looper.getMainLooper()).idleFor(java.time.Duration.ofMillis(100));
    }
}

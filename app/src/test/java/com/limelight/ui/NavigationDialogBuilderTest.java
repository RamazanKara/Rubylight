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
}

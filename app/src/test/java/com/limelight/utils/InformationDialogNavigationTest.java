package com.limelight.utils;

import android.app.Application;
import android.view.KeyEvent;

import androidx.appcompat.app.AlertDialog;

import com.limelight.ui.UiNavigationTest;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;

import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class InformationDialogNavigationTest {
    @Test public void backAndControllerBCloseInformationAndRunItsCompletionOnce() {
        ActivityController<UiNavigationTest.Screen> activity = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup();
        try {
            for (int key : new int[] {KeyEvent.KEYCODE_BUTTON_B, KeyEvent.KEYCODE_ESCAPE}) {
                AtomicInteger completed = new AtomicInteger();
                Dialog.displayDialog(activity.get(), "Details", "Example game", completed::incrementAndGet);
                Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
                AlertDialog dialog = (AlertDialog) ShadowDialog.getLatestDialog();
                dialog.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_DOWN, key));
                dialog.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_UP, key));
                Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
                assertFalse(dialog.isShowing());
                assertEquals(1, completed.get());
                Dialog.closeDialogs(activity.get());
                assertEquals(1, completed.get());
            }
        } finally { activity.destroy(); }
    }
}

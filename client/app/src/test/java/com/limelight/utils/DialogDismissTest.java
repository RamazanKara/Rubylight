package com.limelight.utils;

import android.app.Activity;
import android.os.Looper;

import androidx.appcompat.app.AlertDialog;

import com.limelight.R;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.util.ReflectionHelpers;

import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

/**
 * Dialog keeps every dialog in a static list that holds its Activity: a dialog the framework closes on its
 * own must leave that list and run its dismiss action, exactly as one closed with a button does.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = android.app.Application.class)
public class DialogDismissTest {
    private static Activity activity() {
        Activity activity = Robolectric.buildActivity(Activity.class).setup().get();
        activity.setTheme(R.style.AppTheme);
        return activity;
    }

    private static AlertDialog latest() {
        return (AlertDialog) ShadowDialog.getLatestDialog();
    }

    @SuppressWarnings("unchecked")
    private static List<Dialog> listed() {
        return (List<Dialog>) ReflectionHelpers.getStaticField(Dialog.class, "rundownDialogs");
    }

    @Test
    public void aDialogTheFrameworkClosesStillRunsItsDismissAction() {
        Activity activity = activity();
        AtomicInteger dismissed = new AtomicInteger();
        Dialog.displayDialog(activity, "Title", "Message", dismissed::incrementAndGet);
        shadowOf(Looper.getMainLooper()).idle();
        AlertDialog alert = latest();
        assertNotNull(alert);
        assertTrue(alert.isShowing());
        assertEquals(1, listed().size());
        assertEquals(0, dismissed.get());

        // No button and no Back: what the framework or a plain dismiss() does.
        alert.dismiss();
        shadowOf(Looper.getMainLooper()).idle();
        assertEquals(1, dismissed.get());
        assertTrue("the Activity is no longer held", listed().isEmpty());

        // The action counts once, even if the dialog is dismissed again.
        alert.dismiss();
        shadowOf(Looper.getMainLooper()).idle();
        assertEquals(1, dismissed.get());
        assertFalse(alert.isShowing());
    }

    @Test
    public void closingTheScreensDialogsDoesNotRunTheirDismissAction() {
        Activity activity = activity();
        AtomicInteger dismissed = new AtomicInteger();
        Dialog.displayDialog(activity, "Title", "Message", dismissed::incrementAndGet);
        shadowOf(Looper.getMainLooper()).idle();
        AlertDialog alert = latest();
        assertTrue(alert.isShowing());
        assertEquals(1, listed().size());

        Dialog.closeDialogs(activity);
        shadowOf(Looper.getMainLooper()).idle();
        assertFalse(alert.isShowing());
        assertTrue(listed().isEmpty());
        assertEquals("a screen closing its dialogs is not the user dismissing one", 0, dismissed.get());
    }
}

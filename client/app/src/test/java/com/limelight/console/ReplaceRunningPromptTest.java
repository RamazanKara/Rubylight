package com.limelight.console;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.DialogInterface;
import android.view.ContextThemeWrapper;

import androidx.appcompat.app.AlertDialog;

import com.limelight.R;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.util.concurrent.atomic.AtomicInteger;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class ReplaceRunningPromptTest {
    private final ContextThemeWrapper context = new ContextThemeWrapper(RuntimeEnvironment.getApplication(), R.style.AppTheme);

    private AlertDialog show(Runnable resume, Runnable replace) {
        AlertDialog dialog = ReplaceRunningPrompt.build(context, "Zelda", "Portal", resume, replace);
        dialog.show();
        idle();
        return dialog;
    }

    private static void idle() {
        org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
    }

    @Test
    public void tappingAnotherGameOffersResumeCloseAndStartOrCancel() {
        AlertDialog dialog = show(() -> { }, () -> { });
        assertEquals("Resume Zelda", dialog.getButton(DialogInterface.BUTTON_NEUTRAL).getText().toString());
        assertEquals("Close Zelda and start Portal",
                dialog.getButton(DialogInterface.BUTTON_POSITIVE).getText().toString());
        assertEquals(context.getString(android.R.string.cancel),
                dialog.getButton(DialogInterface.BUTTON_NEGATIVE).getText().toString());
    }

    @Test
    public void theMessageSaysWhatRunsAndWhatStartingDoes() {
        AlertDialog dialog = show(() -> { }, () -> { });
        String message = ((android.widget.TextView) dialog.findViewById(android.R.id.message)).getText().toString();
        assertTrue(message, message.contains("Zelda") && message.contains("Portal"));
        assertTrue(message, message.contains("closes") && message.contains("unsaved"));
    }

    @Test
    public void focusStartsOnResumeNeverOnTheButtonThatClosesTheGame() {
        AlertDialog dialog = show(() -> { }, () -> { });
        assertTrue(dialog.getButton(DialogInterface.BUTTON_NEUTRAL).hasFocus());
        assertFalse(dialog.getButton(DialogInterface.BUTTON_POSITIVE).hasFocus());
    }

    @Test
    public void withNothingToResumeThereIsNoResumeButtonAndFocusStartsOnCancel() {
        AlertDialog dialog = show(null, () -> { });
        assertEquals(android.view.View.GONE, dialog.getButton(DialogInterface.BUTTON_NEUTRAL).getVisibility());
        assertTrue(dialog.getButton(DialogInterface.BUTTON_NEGATIVE).hasFocus());
        assertFalse(dialog.getButton(DialogInterface.BUTTON_POSITIVE).hasFocus());
    }

    @Test
    public void eachButtonDoesOnlyItsOwnThing() {
        AtomicInteger resumed = new AtomicInteger();
        AtomicInteger replaced = new AtomicInteger();
        AlertDialog dialog = show(resumed::incrementAndGet, replaced::incrementAndGet);
        dialog.getButton(DialogInterface.BUTTON_NEUTRAL).performClick();
        idle();
        assertEquals(1, resumed.get());
        assertEquals(0, replaced.get());
        dialog = show(resumed::incrementAndGet, replaced::incrementAndGet);
        dialog.getButton(DialogInterface.BUTTON_POSITIVE).performClick();
        idle();
        assertEquals(1, resumed.get());
        assertEquals(1, replaced.get());
        dialog = show(resumed::incrementAndGet, replaced::incrementAndGet);
        dialog.getButton(DialogInterface.BUTTON_NEGATIVE).performClick();
        idle();
        assertEquals(1, resumed.get());
        assertEquals(1, replaced.get());
    }
}

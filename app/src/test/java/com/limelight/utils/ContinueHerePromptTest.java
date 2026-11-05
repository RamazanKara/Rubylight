package com.limelight.utils;

import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.DialogInterface;
import android.os.Looper;

import androidx.appcompat.app.AlertDialog;
import androidx.test.platform.app.InstrumentationRegistry;

import com.limelight.nativeapi.StreamMoveFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.ui.ToolbarIconTintTest;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;

/** A pad's first A presses Continue here, however the player got to the prompt. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class ContinueHerePromptTest {
    private static void assertContinueHereHasTheFocus(boolean touchMode) {
        InstrumentationRegistry.getInstrumentation().setInTouchMode(touchMode);
        try (ActivityController<ToolbarIconTintTest.Screen> controller =
                     Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup().visible()) {
            AlertDialog dialog = ContinueHereGate.prompt(controller.get(), new ComputerDetails(), null,
                    StreamMoveFixtures.playing("Phone", 7), from -> { });
            dialog.show();
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            assertTrue(dialog.getButton(DialogInterface.BUTTON_POSITIVE).isFocused());
            dialog.dismiss();
        }
    }

    @Test
    public void afterATouch() {
        assertContinueHereHasTheFocus(true);
    }

    @Test
    public void afterAPadPress() {
        assertContinueHereHasTheFocus(false);
    }
}

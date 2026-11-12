package com.limelight.utils;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.DialogInterface;
import android.os.Looper;
import android.view.View;
import android.view.ViewGroup;
import android.view.WindowManager;

import androidx.appcompat.app.AlertDialog;
import androidx.test.platform.app.InstrumentationRegistry;

import com.limelight.nativeapi.StreamMoveFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.ui.NightSkyView;
import com.limelight.ui.ToolbarIconTintTest;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;

import java.time.Duration;

/** A pad's first A presses Continue here, however the player got to the prompt; on a TV the prompt stands on the sky. */
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

    /**
     * On a Shield the prompt stood over the game's page with its buttons on the line of the page's Play, Favourite and Quit
     * game, and Quit game showed just past the panel's edge, as if the panel cut it. On a TV it stands on the sky, without a
     * dim, keeps Continue here focused, and gives the page back when it goes.
     */
    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void onATvThePromptStandsOnTheSkyNotOverThePagesButtons() {
        try (ActivityController<ToolbarIconTintTest.Screen> controller =
                     Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup().visible()) {
            ViewGroup content = controller.get().findViewById(android.R.id.content);
            View page = content.getChildAt(0);
            assertEquals(1, content.getChildCount());
            AlertDialog dialog = ContinueHereGate.prompt(controller.get(), new ComputerDetails(), null,
                    StreamMoveFixtures.playing("Phone", 7), from -> { });
            dialog.show();
            frame();
            assertEquals("the sky covers the page", 2, content.getChildCount());
            assertTrue(content.getChildAt(1) instanceof NightSkyView);
            assertEquals(View.IMPORTANT_FOR_ACCESSIBILITY_NO_HIDE_DESCENDANTS, page.getImportantForAccessibility());
            assertEquals("no dim over the sky", 0,
                    dialog.getWindow().getAttributes().flags & WindowManager.LayoutParams.FLAG_DIM_BEHIND);
            assertTrue(dialog.getButton(DialogInterface.BUTTON_POSITIVE).isFocused());
            dialog.dismiss();
            frame();
            assertEquals("the page is back", 1, content.getChildCount());
        }
    }

    /** A phone's prompt stays over its dimmed page, as every other dialog there. */
    @Test
    public void onAPhoneThePromptStaysOverItsPage() {
        try (ActivityController<ToolbarIconTintTest.Screen> controller =
                     Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup().visible()) {
            ViewGroup content = controller.get().findViewById(android.R.id.content);
            AlertDialog dialog = ContinueHereGate.prompt(controller.get(), new ComputerDetails(), null,
                    StreamMoveFixtures.playing("Phone", 7), from -> { });
            dialog.show();
            frame();
            assertEquals(1, content.getChildCount());
            assertEquals(View.IMPORTANT_FOR_ACCESSIBILITY_AUTO, content.getChildAt(0).getImportantForAccessibility());
            dialog.dismiss();
        }
    }

    /** A window's first traversal attaches its views; it waits for the next frame. */
    private static void frame() {
        Shadows.shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMillis(100));
    }
}

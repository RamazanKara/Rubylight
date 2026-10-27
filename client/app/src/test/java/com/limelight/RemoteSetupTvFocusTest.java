package com.limelight;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertSame;

import android.app.Application;
import android.view.KeyEvent;
import android.view.View;

import com.limelight.ui.TvScreensPreviewTest;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;

/**
 * Play away from home with a TV remote (nightjar#7): the first focus is Next, never the empty result line, and moving
 * between steps keeps focus on the step's action when the focused button hides or disables.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class, qualifiers = "w960dp-h540dp-land-television-xhdpi",
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
@GraphicsMode(GraphicsMode.Mode.NATIVE)
public class RemoteSetupTvFocusTest {
    @Test
    public void theFirstFocusIsNextAndTheEmptyResultIsNeverAStop() {
        try (ActivityController<RemoteSetupActivityTest.OfflineActivity> controller =
                     Robolectric.buildActivity(RemoteSetupActivityTest.OfflineActivity.class).setup().visible()) {
            RemoteSetupActivity screen = controller.get();
            TvScreensPreviewTest.leaveTouchMode(screen);
            TvScreensPreviewTest.settle(screen);
            View next = screen.findViewById(R.id.remote_next);
            View result = screen.findViewById(R.id.remote_result);
            assertFalse("an empty result is not a focus stop", result.isFocusable());
            assertSame(next, focus(screen));
            // Down and Up around Next never stop on the empty line.
            for (int key : new int[] {KeyEvent.KEYCODE_DPAD_DOWN, KeyEvent.KEYCODE_DPAD_UP, KeyEvent.KEYCODE_DPAD_DOWN}) {
                TvScreensPreviewTest.key(screen, key);
                TvScreensPreviewTest.settle(screen);
                View now = focus(screen);
                assertFalse("focus left for the empty result", now == result);
            }
        }
    }

    @Test
    public void stepsKeepFocusOnTheirActionWhenTheFocusedButtonGoes() {
        try (ActivityController<RemoteSetupActivityTest.OfflineActivity> controller =
                     Robolectric.buildActivity(RemoteSetupActivityTest.OfflineActivity.class).setup().visible()) {
            RemoteSetupActivity screen = controller.get();
            TvScreensPreviewTest.leaveTouchMode(screen);
            TvScreensPreviewTest.settle(screen);
            View next = screen.findViewById(R.id.remote_next);
            View previous = screen.findViewById(R.id.remote_previous);
            TvScreensPreviewTest.key(screen, KeyEvent.KEYCODE_DPAD_CENTER);
            TvScreensPreviewTest.settle(screen);
            assertSame("step 2 keeps Next", next, focus(screen));
            TvScreensPreviewTest.key(screen, KeyEvent.KEYCODE_DPAD_CENTER);
            TvScreensPreviewTest.settle(screen);
            assertEquals(View.GONE, next.getVisibility());
            // Offline, the check can't run, so the last step leads with the address.
            assertSame("Next hid: focus moves to the step's action", screen.findViewById(R.id.remote_address), focus(screen));
            // Back to the first step from Previous: Previous disables there, focus moves to Next.
            previous.requestFocus();
            TvScreensPreviewTest.key(screen, KeyEvent.KEYCODE_DPAD_CENTER);
            TvScreensPreviewTest.settle(screen);
            previous.requestFocus();
            TvScreensPreviewTest.key(screen, KeyEvent.KEYCODE_DPAD_CENTER);
            TvScreensPreviewTest.settle(screen);
            assertFalse(previous.isEnabled());
            assertSame("Previous disabled: focus moves to Next", next, focus(screen));
        }
    }

    private static View focus(RemoteSetupActivity screen) {
        return screen.getWindow().getDecorView().findFocus();
    }
}

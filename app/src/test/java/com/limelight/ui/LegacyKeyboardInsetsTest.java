package com.limelight.ui;

import android.app.Activity;
import android.app.Application;
import android.os.Looper;
import android.view.View;
import android.view.WindowManager;
import android.widget.PopupWindow;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

/** The soft keyboard on Android 10 and older, measured by an invisible resizing popup. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class LegacyKeyboardInsetsTest {
    private final List<String> heard = new ArrayList<>();

    @Test
    public void aBarIsNoKeyboard() {
        // Landscape phone, 1080 px: a 132 px navigation bar under the content is no keyboard.
        assertEquals(0, LegacyKeyboardInsets.covered(1080, 948, 1080));
        assertEquals(500, LegacyKeyboardInsets.covered(1080, 580, 1080));
        assertEquals("nothing covered", 0, LegacyKeyboardInsets.covered(1080, 1080, 1080));
        assertEquals("a visible part below the content", 0, LegacyKeyboardInsets.covered(1000, 1080, 1080));
        assertEquals("not laid out", 0, LegacyKeyboardInsets.covered(1080, 0, 0));
    }

    @Test
    public void theListenerHearsEachChangeOnce() {
        Activity activity = Robolectric.buildActivity(Activity.class).setup().get();
        LegacyKeyboardInsets insets = new LegacyKeyboardInsets(activity, activity.findViewById(android.R.id.content),
                (visible, bottom) -> heard.add(visible + " " + bottom));
        insets.onFrame(2400, 2400, 2400);
        assertTrue("hidden as it starts", heard.isEmpty());
        insets.onFrame(2400, 1400, 2400);
        insets.onFrame(2400, 1400, 2400);
        assertTrue(insets.visible());
        // The suggestion strip came up: taller.
        insets.onFrame(2400, 1300, 2400);
        insets.onFrame(2400, 2300, 2400);
        assertFalse(insets.visible());
        assertEquals(Arrays.asList("true 1000", "true 1100", "false 0"), heard);
    }

    @Test
    public void theProbeIsAnInvisibleResizingPopupShownOnceTheContentIsInAWindow() {
        Activity activity = Robolectric.buildActivity(Activity.class).setup().get();
        View content = activity.findViewById(android.R.id.content);
        LegacyKeyboardInsets insets = new LegacyKeyboardInsets(activity, content, (visible, bottom) -> { });
        insets.start();
        shadowOf(Looper.getMainLooper()).idle();
        PopupWindow popup = shadowOf(RuntimeEnvironment.getApplication()).getLatestPopupWindow();
        assertNotNull(popup);
        assertTrue(popup.isShowing());
        assertFalse("never takes a touch from the stream", popup.isTouchable());
        assertFalse("never takes the focus or the keyboard from the stream", popup.isFocusable());
        assertEquals(0, popup.getWidth());
        assertEquals(PopupWindow.INPUT_METHOD_NEEDED, popup.getInputMethodMode());
        assertEquals(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE,
                popup.getSoftInputMode() & WindowManager.LayoutParams.SOFT_INPUT_MASK_ADJUST);
        insets.stop();
        assertFalse(popup.isShowing());
    }

    @Test
    public void startedBeforeTheContentIsAttachedItShowsOnAttach() {
        Activity activity = Robolectric.buildActivity(Activity.class).setup().get();
        View detached = new View(activity);
        LegacyKeyboardInsets insets = new LegacyKeyboardInsets(activity, detached, (visible, bottom) -> { });
        insets.start();
        shadowOf(Looper.getMainLooper()).idle();
        assertNull(shadowOf(RuntimeEnvironment.getApplication()).getLatestPopupWindow());
        activity.setContentView(detached);
        shadowOf(Looper.getMainLooper()).idle();
        PopupWindow popup = shadowOf(RuntimeEnvironment.getApplication()).getLatestPopupWindow();
        assertNotNull(popup);
        assertTrue(popup.isShowing());
        insets.stop();
        assertFalse(popup.isShowing());
    }
}

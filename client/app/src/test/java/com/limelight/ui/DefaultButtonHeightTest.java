package com.limelight.ui;

import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;
import android.view.ContextThemeWrapper;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

/**
 * A button without a style of its own, as the settings' sub-screens use, takes the theme's materialButtonStyle; it
 * stands as tall as the tonal and text buttons beside it (the touch target), not Material's 40dp, on a TV too.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class)
public class DefaultButtonHeightTest {
    private static void assertTouchTarget(String where) {
        Context context = new ContextThemeWrapper(RuntimeEnvironment.getApplication(), R.style.Base_Theme_Nightjar);
        int target = context.getResources().getDimensionPixelSize(R.dimen.touch_target);
        for (int style : new int[] {0, R.style.Widget_Nightjar_Button_Tonal, R.style.Widget_Nightjar_Button_Text}) {
            MaterialButton button = style == 0 ? new MaterialButton(context) : new MaterialButton(new ContextThemeWrapper(context, style), null, 0);
            assertTrue(where + " style " + style + ": " + button.getMinimumHeight() + " < " + target, button.getMinimumHeight() >= target);
        }
    }

    @Test
    public void aPlainButtonIsTouchTargetTallOnAPhone() {
        assertTouchTarget("phone");
    }

    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void aPlainButtonIsTouchTargetTallOnATv() {
        assertTouchTarget("tv");
    }
}

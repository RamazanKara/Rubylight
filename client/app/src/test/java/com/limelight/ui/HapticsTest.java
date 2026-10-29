package com.limelight.ui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotEquals;
import static org.robolectric.Shadows.shadowOf;

import android.content.Context;
import android.os.SystemClock;
import android.view.HapticFeedbackConstants;
import android.view.ContextThemeWrapper;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.FrameLayout;

import com.limelight.R;
import com.limelight.binding.input.keyboard.PcKeyboardController;
import com.limelight.binding.input.keyboard.PcKeyboardLayout;
import com.limelight.preferences.MaterialSwitchPreference;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

/** Touch feedback on the PC keyboard, sliders, settings switches and the shell bar (nightjar#44). */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34)
public class HapticsTest {
    @Test
    public void eachKindTakesTheNewestConstantTheDeviceKnows() {
        assertEquals(HapticFeedbackConstants.KEYBOARD_TAP, Haptics.constant(Haptics.Kind.KEY, 21));
        assertEquals(HapticFeedbackConstants.KEYBOARD_TAP, Haptics.constant(Haptics.Kind.KEY, 34));
        assertEquals(HapticFeedbackConstants.VIRTUAL_KEY, Haptics.constant(Haptics.Kind.NAVIGATION, 21));
        assertEquals(HapticFeedbackConstants.VIRTUAL_KEY, Haptics.constant(Haptics.Kind.NAVIGATION, 36));

        assertEquals(HapticFeedbackConstants.CLOCK_TICK, Haptics.constant(Haptics.Kind.STEP, 21));
        assertEquals(HapticFeedbackConstants.CLOCK_TICK, Haptics.constant(Haptics.Kind.STEP, 33));
        assertEquals(HapticFeedbackConstants.SEGMENT_FREQUENT_TICK, Haptics.constant(Haptics.Kind.STEP, 34));

        assertEquals(HapticFeedbackConstants.VIRTUAL_KEY, Haptics.constant(Haptics.Kind.TOGGLE_ON, 33));
        assertEquals(HapticFeedbackConstants.VIRTUAL_KEY, Haptics.constant(Haptics.Kind.TOGGLE_OFF, 33));
        assertEquals(HapticFeedbackConstants.TOGGLE_ON, Haptics.constant(Haptics.Kind.TOGGLE_ON, 34));
        assertEquals(HapticFeedbackConstants.TOGGLE_OFF, Haptics.constant(Haptics.Kind.TOGGLE_OFF, 34));
    }

    @Test
    public void aKeyOfThePcKeyboardTapsWhenItGoesDown() {
        Context context = new ContextThemeWrapper(RuntimeEnvironment.getApplication(), R.style.AppTheme);
        PcKeyboardController controller = new PcKeyboardController(PcKeyboardLayout.byId(PcKeyboardLayout.US),
                (vk, down, modifiers) -> { },
                new PcKeyboardController.Host() {
                    @Override public void close() { }
                    @Override public void changed() { }
                });
        PcKeyboardView keys = new PcKeyboardView(context, controller);
        keys.measure(View.MeasureSpec.makeMeasureSpec(1000, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(400, View.MeasureSpec.EXACTLY));
        keys.layout(0, 0, 1000, 400);
        assertNotEquals(HapticFeedbackConstants.KEYBOARD_TAP, shadowOf(keys).lastHapticFeedbackPerformed());

        long now = SystemClock.uptimeMillis();
        MotionEvent down = MotionEvent.obtain(now, now, MotionEvent.ACTION_DOWN, 500, 200, 0);
        keys.onTouchEvent(down);
        down.recycle();
        assertEquals(HapticFeedbackConstants.KEYBOARD_TAP, shadowOf(keys).lastHapticFeedbackPerformed());
    }

    @Test
    public void aSettingsSwitchAnswersOnAndOff() {
        Context context = new ContextThemeWrapper(RuntimeEnvironment.getApplication(), R.style.AppTheme);
        android.preference.PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
        TestSwitch toggle = new TestSwitch(context);
        toggle.setKey("haptics_test_switch");
        toggle.setChecked(false);
        ViewGroup parent = new FrameLayout(context);
        View row = toggle.getView(null, parent);

        toggle.tap();
        assertEquals(HapticFeedbackConstants.TOGGLE_ON, shadowOf(row).lastHapticFeedbackPerformed());
        toggle.tap();
        assertEquals(HapticFeedbackConstants.TOGGLE_OFF, shadowOf(row).lastHapticFeedbackPerformed());
    }

    @Test
    public void aSwitchWhoseChangeIsRefusedStaysQuiet() {
        Context context = new ContextThemeWrapper(RuntimeEnvironment.getApplication(), R.style.AppTheme);
        TestSwitch toggle = new TestSwitch(context);
        toggle.setChecked(false);
        toggle.setOnPreferenceChangeListener((preference, value) -> false);
        View row = toggle.getView(null, new FrameLayout(context));

        toggle.tap();
        assertFalse(toggle.isChecked());
        assertNotEquals(HapticFeedbackConstants.TOGGLE_ON, shadowOf(row).lastHapticFeedbackPerformed());
        assertNotEquals(HapticFeedbackConstants.TOGGLE_OFF, shadowOf(row).lastHapticFeedbackPerformed());
    }

    /** The preference's own tap, as the settings list delivers it. */
    static final class TestSwitch extends MaterialSwitchPreference {
        TestSwitch(Context context) {
            super(context, null);
            setPersistent(false);
        }

        void tap() {
            onClick();
        }
    }
}

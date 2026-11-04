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
        assertEquals(HapticFeedbackConstants.VIRTUAL_KEY, Haptics.constant(Haptics.Kind.KEY, 21));
        assertEquals(HapticFeedbackConstants.VIRTUAL_KEY, Haptics.constant(Haptics.Kind.KEY, 34));
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
    public void theWasdStickHasADirectionAndAClearlyDifferentDiagonal() {
        assertEquals(HapticFeedbackConstants.CLOCK_TICK, Haptics.constant(Haptics.Kind.DIRECTION, 21));
        assertEquals(HapticFeedbackConstants.CONTEXT_CLICK, Haptics.constant(Haptics.Kind.DIRECTION, 23));
        assertEquals(HapticFeedbackConstants.CONTEXT_CLICK, Haptics.constant(Haptics.Kind.DIRECTION, 36));
        assertEquals(HapticFeedbackConstants.VIRTUAL_KEY, Haptics.constant(Haptics.Kind.DIAGONAL, 21));
        assertEquals(HapticFeedbackConstants.VIRTUAL_KEY, Haptics.constant(Haptics.Kind.DIAGONAL, 36));
        for (int sdk = 21; sdk <= 36; sdk++) {
            assertNotEquals("API " + sdk, Haptics.constant(Haptics.Kind.DIRECTION, sdk), Haptics.constant(Haptics.Kind.DIAGONAL, sdk));
        }
        assertEquals(HapticFeedbackConstants.VIRTUAL_KEY, Haptics.constant(Haptics.Kind.CLICK, 21));
        assertEquals(HapticFeedbackConstants.VIRTUAL_KEY, Haptics.constant(Haptics.Kind.CLICK, 36));
    }

    @Test
    public void thePcKeyboardFollowsVibrateOnPress() {
        Context context = new ContextThemeWrapper(RuntimeEnvironment.getApplication(), R.style.AppTheme);
        PcKeyboardController controller = new PcKeyboardController(PcKeyboardLayout.byId(PcKeyboardLayout.US),
                (vk, down, modifiers) -> { },
                new PcKeyboardController.Host() {
                    @Override public void close() { }
                    @Override public void changed() { }
                });
        for (boolean haptics : new boolean[] {true, false}) {
            PcKeyboardView keys = new PcKeyboardView(context, controller);
            keys.setHaptics(haptics);
            keys.measure(View.MeasureSpec.makeMeasureSpec(1000, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(400, View.MeasureSpec.EXACTLY));
            keys.layout(0, 0, 1000, 400);
            long now = SystemClock.uptimeMillis();
            MotionEvent down = MotionEvent.obtain(now, now, MotionEvent.ACTION_DOWN, 500, 200, 0);
            keys.onTouchEvent(down);
            down.recycle();
            if (haptics) {
                assertEquals(HapticFeedbackConstants.VIRTUAL_KEY, shadowOf(keys).lastHapticFeedbackPerformed());
            } else {
                assertNotEquals(HapticFeedbackConstants.VIRTUAL_KEY, shadowOf(keys).lastHapticFeedbackPerformed());
            }
            MotionEvent up = MotionEvent.obtain(now, now + 50, MotionEvent.ACTION_UP, 500, 200, 0);
            keys.onTouchEvent(up);
            up.recycle();
        }
    }

    @Test
    public void aTrackpadTapClicksAndFollowsVibrateOnPress() {
        Context context = new ContextThemeWrapper(RuntimeEnvironment.getApplication(), R.style.AppTheme);
        java.util.List<String> sent = new java.util.ArrayList<>();
        for (boolean haptics : new boolean[] {true, false}) {
            com.limelight.binding.input.DesktopControlsConfig config = com.limelight.binding.input.DesktopControlsConfig.load(context);
            config.tapToClick = true;
            DesktopTrackpad pad = new DesktopTrackpad(context, config, new DesktopTrackpad.Sink() {
                @Override public void move(float dx, float dy) { }
                @Override public void scroll(float vertical, float horizontal) { }
                @Override public void mouseButton(int button, boolean down) { sent.add(button + (down ? "+" : "-")); }
            });
            pad.setHaptics(haptics);
            pad.measure(View.MeasureSpec.makeMeasureSpec(600, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(300, View.MeasureSpec.EXACTLY));
            pad.layout(0, 0, 600, 300);
            // Moving the pointer alone is quiet.
            long now = SystemClock.uptimeMillis();
            MotionEvent down = MotionEvent.obtain(now, now, MotionEvent.ACTION_DOWN, 300, 150, 0);
            pad.onTouchEvent(down);
            down.recycle();
            assertNotEquals(HapticFeedbackConstants.VIRTUAL_KEY, shadowOf(pad).lastHapticFeedbackPerformed());
            MotionEvent up = MotionEvent.obtain(now, now + 80, MotionEvent.ACTION_UP, 300, 150, 0);
            pad.onTouchEvent(up);
            up.recycle();
            assertEquals("A tap clicks", "1+", sent.get(sent.size() - 1));
            if (haptics) {
                assertEquals(HapticFeedbackConstants.VIRTUAL_KEY, shadowOf(pad).lastHapticFeedbackPerformed());
            } else {
                assertNotEquals(HapticFeedbackConstants.VIRTUAL_KEY, shadowOf(pad).lastHapticFeedbackPerformed());
            }
            pad.releaseInputs();
        }
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
        assertNotEquals(HapticFeedbackConstants.VIRTUAL_KEY, shadowOf(keys).lastHapticFeedbackPerformed());

        long now = SystemClock.uptimeMillis();
        MotionEvent down = MotionEvent.obtain(now, now, MotionEvent.ACTION_DOWN, 500, 200, 0);
        keys.onTouchEvent(down);
        down.recycle();
        assertEquals(HapticFeedbackConstants.VIRTUAL_KEY, shadowOf(keys).lastHapticFeedbackPerformed());
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

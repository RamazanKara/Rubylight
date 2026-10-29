package com.limelight.preferences;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertNotNull;
import static org.robolectric.Shadows.shadowOf;

import android.content.Context;
import android.view.ContextThemeWrapper;
import android.view.HapticFeedbackConstants;
import android.view.KeyEvent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.SeekBar;

import com.limelight.R;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

/** The bitrate slider ticks once per value it moves to, from a drag or a button (nightjar#44). */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34)
public class SeekBarPreferenceHapticsTest {
    private static final int STEP = HapticFeedbackConstants.SEGMENT_FREQUENT_TICK;

    private View dialog() {
        Context context = new ContextThemeWrapper(RuntimeEnvironment.getApplication(), R.style.AppTheme);
        SeekBarPreference bitrate = new SeekBarPreference(context, Robolectric.buildAttributeSet()
                .addAttribute(android.R.attr.max, "100000")
                .addAttribute(android.R.attr.defaultValue, "20000")
                .build());
        bitrate.setPersistent(false);
        return bitrate.onCreateDialogView();
    }

    private static <T extends View> T find(View view, Class<T> type, CharSequence description) {
        if (type.isInstance(view) && (description == null || description.equals(view.getContentDescription()))) {
            return type.cast(view);
        }
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) {
                T found = find(group.getChildAt(i), type, description);
                if (found != null) return found;
            }
        }
        return null;
    }

    @Test
    public void plusAndMinusTickWhenTheValueMoves() {
        View dialog = dialog();
        Context context = dialog.getContext();
        View plus = find(dialog, View.class, context.getString(R.string.seekbar_increase));
        assertNotNull(plus);
        plus.performClick();
        assertEquals(STEP, shadowOf(plus).lastHapticFeedbackPerformed());
    }

    @Test
    public void minusAtTheBottomStaysQuiet() {
        Context context = new ContextThemeWrapper(RuntimeEnvironment.getApplication(), R.style.AppTheme);
        SeekBarPreference small = new SeekBarPreference(context, Robolectric.buildAttributeSet()
                .addAttribute(android.R.attr.max, "10")
                .addAttribute(android.R.attr.defaultValue, "1")
                .build());
        small.setPersistent(false);
        View dialog = small.onCreateDialogView();
        View minus = find(dialog, View.class, context.getString(R.string.seekbar_decrease));
        assertNotNull(minus);
        minus.performClick();
        assertNotEquals(STEP, shadowOf(minus).lastHapticFeedbackPerformed());
    }

    @Test
    public void movingTheSliderTicks() {
        View dialog = dialog();
        SeekBar slider = find(dialog, SeekBar.class, null);
        assertNotNull(slider);
        slider.onKeyDown(KeyEvent.KEYCODE_DPAD_RIGHT, new KeyEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_RIGHT));
        assertEquals(STEP, shadowOf(slider).lastHapticFeedbackPerformed());
    }
}

package com.limelight.preferences;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertTrue;

import android.content.Context;
import android.content.res.XmlResourceParser;
import android.util.AttributeSet;
import android.util.Xml;
import android.view.KeyEvent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.TextView;

import com.google.android.material.slider.LabelFormatter;
import com.google.android.material.slider.Slider;
import com.limelight.R;
import com.limelight.ui.StepSlider;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.xmlpull.v1.XmlPullParser;

import java.util.ArrayList;
import java.util.List;

/**
 * nightjar#61: every slider setting opens the thick Material slider, with its stored value, bounds,
 * key step and value text as they were with the platform SeekBar.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34)
public class SeekBarPreferenceSliderTest {
    private static final String ANDROID = "http://schemas.android.com/apk/res/android";
    private static final String SEEKBAR = "http://schemas.moonlight-stream.com/apk/res/seekbar";

    @Test
    public void everySliderSettingKeepsItsValueBoundsAndSteps() throws Exception {
        Context context = new android.view.ContextThemeWrapper(RuntimeEnvironment.getApplication(), R.style.AppTheme);
        List<String> keys = new ArrayList<>();
        try (XmlResourceParser parser = context.getResources().getXml(R.xml.preferences)) {
            for (int event = parser.getEventType(); event != XmlPullParser.END_DOCUMENT; event = parser.next()) {
                if (event != XmlPullParser.START_TAG || !parser.getName().equals(SeekBarPreference.class.getName())) {
                    continue;
                }
                AttributeSet attrs = Xml.asAttributeSet(parser);
                String key = attrs.getAttributeValue(ANDROID, "key");
                int max = attrs.getAttributeIntValue(ANDROID, "max", 100);
                int min = attrs.getAttributeIntValue(SEEKBAR, "min", 1);
                int step = attrs.getAttributeIntValue(SEEKBAR, "step", 1);
                int keyStep = attrs.getAttributeIntValue(SEEKBAR, "keyStep", 0);
                boolean log = attrs.getAttributeBooleanValue(SEEKBAR, "logScale", false);
                SeekBarPreference pref = new SeekBarPreference(context, attrs);
                keys.add(key);
                check(key, pref, min, max, step, keyStep, log);
            }
        }
        // Issue #61's list: track and cursor speed, mouse speed, vibration, deadzones, opacity, keyboard
        // size and opacity, microphone gain, bitrate, sharpness and PC render resolution.
        for (String key : new String[] {"seekbar_trackpad_speed", "seekbar_local_cursor_speed", "seekbar_mouse_speed",
                "seekbar_vibrate_fallback_strength", "seekbar_deadzone", "seekbar_osc_stick_deadzone",
                "seekbar_osc_opacity", "seekbar_pc_keyboard_size", "seekbar_pc_keyboard_opacity", "seekbar_mic_gain",
                "seekbar_bitrate_kbps", "upscaling_sharpness", "seekbar_virtual_display_scale"}) {
            assertTrue(key + " in " + keys, keys.contains(key));
        }
    }

    private static void check(String key, SeekBarPreference pref, int min, int max, int step, int keyStep,
            boolean log) {
        pref.setPersistent(false);
        // A stored value away from both ends, on the setting's own grid.
        int stored = log ? 20000 : SeekBarPreference.linearPositionToValue((max - min) / 2, min, max, step);
        pref.onSetInitialValue(false, stored);
        String text = pref.getValueText();
        View dialog = pref.onCreateDialogView();
        StepSlider slider = find(dialog, StepSlider.class);
        assertNotNull(key, slider);
        assertTrue(key, slider instanceof Slider);
        assertEquals(key, log ? SeekBarPreference.LOG_POSITIONS : max - min, slider.getMax());
        int position = log ? SeekBarPreference.logValueToPosition(stored, min, max)
                : SeekBarPreference.linearValueToPosition(stored, min, max);
        assertEquals(key, position, slider.getPosition());
        int expectedStep = log ? SeekBarPreference.LOG_POSITIONS / 100 : keyStep != 0 ? keyStep
                : new android.widget.SeekBar(pref.getContext()) {{ setMax(max - min); }}.getKeyProgressIncrement();
        assertEquals(key, expectedStep, slider.getKeyStep());
        assertNotNull(key + " shows " + text, findText(dialog, text));
        assertEquals(key + " shows its value in the dialog's text colour, readable on the glass",
                com.google.android.material.color.MaterialColors.getColor(pref.getContext(),
                        com.google.android.material.R.attr.colorOnSurface, 1),
                findText(dialog, text).getCurrentTextColor());
        assertEquals(key + ": the value over the slider is enough", LabelFormatter.LABEL_GONE, slider.getLabelBehavior());
        assertEquals(key, pref.getTitle(), slider.getContentDescription());

        slider.onKeyDown(KeyEvent.KEYCODE_DPAD_RIGHT, new KeyEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_RIGHT));
        assertEquals(key, Math.min(slider.getMax(), position + expectedStep), slider.getPosition());
        assertEquals(key + " shows the new value", null, findText(dialog, text));
    }

    private static <T extends View> T find(View view, Class<T> type) {
        if (type.isInstance(view)) return type.cast(view);
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) {
                T found = find(group.getChildAt(i), type);
                if (found != null) return found;
            }
        }
        return null;
    }

    private static TextView findText(View view, String text) {
        if (view instanceof TextView && text.contentEquals(((TextView) view).getText())) return (TextView) view;
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) {
                TextView found = findText(group.getChildAt(i), text);
                if (found != null) return found;
            }
        }
        return null;
    }
}

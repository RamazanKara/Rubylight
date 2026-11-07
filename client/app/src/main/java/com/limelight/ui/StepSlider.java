package com.limelight.ui;

import android.content.Context;
import android.content.res.ColorStateList;
import android.util.AttributeSet;
import android.view.KeyEvent;
import android.view.View;

import androidx.core.content.ContextCompat;
import androidx.core.graphics.ColorUtils;

import com.google.android.material.slider.LabelFormatter;
import com.google.android.material.slider.Slider;
import com.limelight.R;

import java.util.function.IntFunction;

/**
 * The app's slider over whole positions from 0 to a maximum, in the thick Material look of the classic
 * controls' opacity slider ({@code osc_opacity_panel}, nightjar#61). It replaces the platform SeekBar in
 * the settings, the stream menu's bitrate dialogs and the desktop controls, and keys step it like that
 * SeekBar did: Left and Right (and minus and plus) move it by {@link #getKeyStep()}, a twentieth of the
 * range unless set, and at an end they leave the key to the window, which moves the focus on. While a
 * finger drags it, and for {@link #KEY_VALUE_MS} after a key step, its value floats over the thumb
 * where the finger does not cover it (nightjar#61).
 */
public class StepSlider extends Slider {
    /** A position the user picked, by touch or by key. */
    public interface OnPositionChangeListener {
        void onPositionChanged(StepSlider slider, int position);
    }

    // The platform SeekBar's key step starts at 1 and only grows for a range wider than 20 steps.
    private static final int SEEKBAR_INITIAL_KEY_STEP = 1;
    /** How long the value stays over the thumb after the last key step. */
    static final long KEY_VALUE_MS = 1500;

    private int keyStep = SEEKBAR_INITIAL_KEY_STEP;
    private OnPositionChangeListener listener;
    private boolean touching;
    private final Runnable hideKeyValue = () -> {
        if (!touching) setActiveThumbIndex(-1);
        invalidate();
    };

    public StepSlider(Context context) {
        this(context, null);
    }

    public StepSlider(Context context, AttributeSet attrs) {
        super(context, attrs);
        setValueFrom(0);
        setValueTo(1);
        setStepSize(1);
        setLabelBehavior(LabelFormatter.LABEL_FLOATING);
        setTickVisible(false);
        int primary = ContextCompat.getColor(context, R.color.night_primary);
        setThumbTintList(ColorStateList.valueOf(primary));
        setTrackActiveTintList(ColorStateList.valueOf(primary));
        setTrackInactiveTintList(ColorStateList.valueOf(ContextCompat.getColor(context, R.color.night_outline_variant)));
        // Material 3 draws no halo; a focused slider shows one so a TV remote can see where it is.
        setHaloTintList(ColorStateList.valueOf(ColorUtils.setAlphaComponent(primary, 0x55)));
        addOnChangeListener((slider, value, fromUser) -> {
            if (fromUser && listener != null) listener.onPositionChanged(this, Math.round(value));
        });
        addOnSliderTouchListener(new OnSliderTouchListener() {
            @Override
            public void onStartTrackingTouch(Slider slider) {
                touching = true;
                removeCallbacks(hideKeyValue);
            }

            @Override
            public void onStopTrackingTouch(Slider slider) {
                touching = false;
            }
        });
    }

    /**
     * Floats {@code slider}'s value, in {@code text}, over its thumb while it moves; TalkBack reads the same
     * text. For the Material sliders that are not a StepSlider.
     */
    public static void showValue(Slider slider, IntFunction<String> text) {
        slider.setLabelBehavior(LabelFormatter.LABEL_FLOATING);
        slider.setLabelFormatter(value -> text.apply(Math.round(value)));
    }

    /** The key step the platform SeekBar picks for a range: 1, or a twentieth of a range wider than 20. */
    static int seekBarKeyStep(int range) {
        int step = SEEKBAR_INITIAL_KEY_STEP;
        if (range / step > 20) {
            step = Math.max(1, Math.round(range / 20f));
        }
        return step;
    }

    public void setMax(int max) {
        int position = Math.min(getPosition(), max);
        setValueTo(Math.max(1, max));
        setValue(position);
        keyStep = seekBarKeyStep(max);
    }

    public int getMax() {
        return Math.round(getValueTo());
    }

    public int getPosition() {
        return Math.round(getValue());
    }

    /** Moves the slider without telling the listener, like SeekBar.setProgress. */
    public void setPosition(int position) {
        setValue(Math.max(0, Math.min(getMax(), position)));
    }

    public int getKeyStep() {
        return keyStep;
    }

    public void setKeyStep(int step) {
        keyStep = Math.abs(step);
    }

    public void setOnPositionChangeListener(OnPositionChangeListener listener) {
        this.listener = listener;
    }

    /** The text a position floats over the thumb while it moves, and what TalkBack reads for it. */
    public void setPositionText(IntFunction<String> text) {
        showValue(this, text);
    }

    /** Whether the value floats over the thumb now. */
    boolean showsValue() {
        return getActiveThumbIndex() != -1;
    }

    @Override
    public boolean onKeyDown(int keyCode, KeyEvent event) {
        if (isEnabled()) {
            int step = keyStep;
            switch (keyCode) {
                case KeyEvent.KEYCODE_DPAD_LEFT:
                case KeyEvent.KEYCODE_MINUS:
                    step = -step;
                    // fall through
                case KeyEvent.KEYCODE_DPAD_RIGHT:
                case KeyEvent.KEYCODE_PLUS:
                case KeyEvent.KEYCODE_EQUALS:
                    if (getLayoutDirection() == View.LAYOUT_DIRECTION_RTL) step = -step;
                    int next = Math.max(0, Math.min(getMax(), getPosition() + step));
                    if (next == getPosition()) {
                        // At an end the window gets the key and moves the focus, as with a SeekBar.
                        return false;
                    }
                    setValue(next);
                    // The key skips Material's own handling, so the value is shown here.
                    setActiveThumbIndex(0);
                    removeCallbacks(hideKeyValue);
                    postDelayed(hideKeyValue, KEY_VALUE_MS);
                    if (listener != null) listener.onPositionChanged(this, next);
                    return true;
                default:
                    break;
            }
        }
        return super.onKeyDown(keyCode, event);
    }
}

package com.limelight.preferences;

import com.limelight.ui.NavigationDialogBuilder;
import com.google.android.material.button.MaterialButton;
import android.content.Context;
import android.os.Bundle;
import android.preference.DialogPreference;
import android.util.AttributeSet;
import android.util.TypedValue;
import android.view.Gravity;
import android.view.View;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.SeekBar;
import android.widget.ScrollView;
import android.widget.TextView;

import com.limelight.R;
import com.limelight.utils.UiHelper;

import java.util.Locale;

// Based on a Stack Overflow example: http://stackoverflow.com/questions/1974193/slider-on-my-preferencescreen
public class SeekBarPreference extends DialogPreference
{
    private static final String ANDROID_SCHEMA_URL = "http://schemas.android.com/apk/res/android";
    private static final String SEEKBAR_SCHEMA_URL = "http://schemas.moonlight-stream.com/apk/res/seekbar";

    // Slider resolution in logarithmic mode
    static final int LOG_POSITIONS = 1000;

    private SeekBar seekBar;
    private TextView valueText;
    private final Context context;

    private final String dialogMessage;
    private final String suffix;
    private final int defaultValue;
    private final int maxValue;
    private final int minValue;
    private final int stepSize;
    private final int keyStepSize;
    private final int divisor;
    private final boolean logScale;
    private final int[] presets;
    private int currentValue;
    // Value shown in the open dialog; persisted only when the user confirms
    private int dialogValue;

    public SeekBarPreference(Context context, AttributeSet attrs) {
        super(context, attrs);
        this.context = context;

        // Read the message from XML
        int dialogMessageId = attrs.getAttributeResourceValue(ANDROID_SCHEMA_URL, "dialogMessage", 0);
        if (dialogMessageId == 0) {
            dialogMessage = getSummary() == null ? null : getSummary().toString();
        }
        else {
            dialogMessage = context.getString(dialogMessageId);
        }

        // Get the suffix for the number displayed in the dialog
        int suffixId = attrs.getAttributeResourceValue(ANDROID_SCHEMA_URL, "text", 0);
        if (suffixId == 0) {
            suffix = attrs.getAttributeValue(ANDROID_SCHEMA_URL, "text");
        }
        else {
            suffix = context.getString(suffixId);
        }

        // Get default, min, and max seekbar values
        defaultValue = attrs.getAttributeIntValue(ANDROID_SCHEMA_URL, "defaultValue", PreferenceConfiguration.getDefaultBitrate(context));
        maxValue = attrs.getAttributeIntValue(ANDROID_SCHEMA_URL, "max", 100);
        minValue = attrs.getAttributeIntValue(SEEKBAR_SCHEMA_URL, "min", 1);
        stepSize = attrs.getAttributeIntValue(SEEKBAR_SCHEMA_URL, "step", 1);
        divisor = attrs.getAttributeIntValue(SEEKBAR_SCHEMA_URL, "divisor", 1);
        keyStepSize = attrs.getAttributeIntValue(SEEKBAR_SCHEMA_URL, "keyStep", 0);
        logScale = attrs.getAttributeBooleanValue(SEEKBAR_SCHEMA_URL, "logScale", false);
        int presetsId = attrs.getAttributeResourceValue(SEEKBAR_SCHEMA_URL, "presets", 0);
        presets = presetsId == 0 ? new int[0] : context.getResources().getIntArray(presetsId);
    }

    private int dp(int value) {
        return Math.round(TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, value,
                context.getResources().getDisplayMetrics()));
    }

    // Logarithmic values snap to a step that stays fine where most streams sit (1 Mbps below 100 Mbps)
    static int logStep(int value) {
        if (value < 10000) {
            return 500;
        }
        else if (value < 100000) {
            return 1000;
        }
        else if (value < 300000) {
            return 5000;
        }
        return 10000;
    }

    static int clamp(int value, int min, int max) {
        return Math.max(min, Math.min(max, value));
    }

    static int positionToLogValue(int position, int min, int max) {
        double raw = min * Math.pow((double) max / min, position / (double) LOG_POSITIONS);
        int step = logStep((int) raw);
        return clamp((int) (Math.round(raw / step) * step), min, max);
    }

    static int logValueToPosition(int value, int min, int max) {
        value = clamp(value, min, max);
        return (int) Math.round(LOG_POSITIONS * Math.log((double) value / min) / Math.log((double) max / min));
    }

    static int nudge(int value, boolean up, boolean logScale, int stepSize, int min, int max) {
        int step = logScale ? logStep(up ? value : value - 1) : stepSize;
        int next = up ? (value / step + 1) * step : ((value - 1) / step) * step;
        return clamp(next, min, max);
    }

    private int positionToValue(int position) {
        if (logScale) {
            return positionToLogValue(position, minValue, maxValue);
        }
        int rounded = ((position + (stepSize - 1)) / stepSize) * stepSize;
        return clamp(rounded, minValue, maxValue);
    }

    private int valueToPosition(int value) {
        return logScale ? logValueToPosition(value, minValue, maxValue) : clamp(value, minValue, maxValue);
    }

    private void showDialogValue(int value) {
        dialogValue = value;
        valueText.setText(formatValue(value));
        int position = valueToPosition(value);
        if (seekBar.getProgress() != position) {
            seekBar.setProgress(position);
        }
    }

    private Button smallButton(String label) {
        Button button = new MaterialButton(context, null, androidx.appcompat.R.attr.borderlessButtonStyle);
        button.setText(label);
        button.setAllCaps(false);
        button.setMinWidth(dp(48));
        button.setMinimumWidth(dp(48));
        button.setMinHeight(dp(48));
        button.setMinimumHeight(dp(48));
        return button;
    }

    @Override
    protected View onCreateDialogView() {
        LinearLayout layout = new LinearLayout(context);
        layout.setOrientation(LinearLayout.VERTICAL);
        layout.setPadding(dp(16), dp(8), dp(16), dp(8));

        if (dialogMessage != null) {
            TextView message = new TextView(context);
            message.setTextAppearance(context, R.style.TextAppearance_Nightjar_Body);
            message.setPadding(dp(8), 0, dp(8), dp(8));
            message.setText(dialogMessage);
            layout.addView(message);
        }

        valueText = new TextView(context);
        valueText.setGravity(Gravity.CENTER_HORIZONTAL);
        valueText.setTextSize(32);
        layout.addView(valueText, new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT));

        LinearLayout sliderRow = new LinearLayout(context);
        sliderRow.setOrientation(LinearLayout.HORIZONTAL);
        sliderRow.setGravity(Gravity.CENTER_VERTICAL);
        Button minus = smallButton("−");
        minus.setTextSize(22);
        minus.setContentDescription(context.getString(R.string.seekbar_decrease));
        minus.setOnClickListener(v -> showDialogValue(nudge(dialogValue, false, logScale, stepSize, minValue, maxValue)));
        Button plus = smallButton("+");
        plus.setTextSize(22);
        plus.setContentDescription(context.getString(R.string.seekbar_increase));
        plus.setOnClickListener(v -> showDialogValue(nudge(dialogValue, true, logScale, stepSize, minValue, maxValue)));

        seekBar = new SeekBar(context);
        seekBar.setMinimumHeight(dp(48));
        seekBar.setContentDescription(getTitle());
        seekBar.setOnSeekBarChangeListener(new SeekBar.OnSeekBarChangeListener() {
            @Override
            public void onProgressChanged(SeekBar seekBar, int position, boolean fromUser) {
                if (fromUser) {
                    dialogValue = positionToValue(position);
                    valueText.setText(formatValue(dialogValue));
                }
            }

            @Override
            public void onStartTrackingTouch(SeekBar seekBar) {}

            @Override
            public void onStopTrackingTouch(SeekBar seekBar) {}
        });
        sliderRow.addView(minus);
        sliderRow.addView(seekBar, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1));
        sliderRow.addView(plus);
        layout.addView(sliderRow, new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT));

        if (presets.length > 0) {
            TextView presetLabel = new TextView(context);
            presetLabel.setTextAppearance(context, R.style.TextAppearance_Nightjar_Label);
            presetLabel.setPadding(dp(8), dp(8), dp(8), 0);
            presetLabel.setText(suffix == null ? context.getString(R.string.seekbar_presets) :
                    context.getString(R.string.seekbar_presets_unit, suffix));
            layout.addView(presetLabel);
            int perRow = 3;
            LinearLayout row = null;
            for (int i = 0; i < presets.length; i++) {
                if (i % perRow == 0) {
                    row = new LinearLayout(context);
                    row.setOrientation(LinearLayout.HORIZONTAL);
                    layout.addView(row, new LinearLayout.LayoutParams(
                            LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT));
                }
                final int preset = clamp(presets[i], minValue, maxValue);
                Button button = smallButton(formatNumber(preset));
                button.setOnClickListener(v -> showDialogValue(preset));
                row.addView(button, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1));
            }
        }

        if (shouldPersist()) {
            currentValue = getPersistedInt(defaultValue);
        }

        seekBar.setMax(logScale ? LOG_POSITIONS : maxValue);
        if (logScale) {
            seekBar.setKeyProgressIncrement(LOG_POSITIONS / 100);
        }
        else if (keyStepSize != 0) {
            seekBar.setKeyProgressIncrement(keyStepSize);
        }
        showDialogValue(clamp(currentValue, minValue, maxValue));

        ScrollView scroll = new ScrollView(context);
        scroll.addView(layout);
        return scroll;
    }

    @Override
    protected void onBindDialogView(View v) {
        super.onBindDialogView(v);
        showDialogValue(clamp(currentValue, minValue, maxValue));
    }

    @Override
    protected void onSetInitialValue(boolean restore, Object defaultValue)
    {
        super.onSetInitialValue(restore, defaultValue);
        if (restore) {
            currentValue = shouldPersist() ? getPersistedInt(this.defaultValue) : 0;
        }
        else {
            currentValue = (Integer) defaultValue;
        }
    }

    // Whole numbers drop the decimal (20 Mbps); fractional ones keep one place (2.5 Mbps)
    private String formatNumber(int value) {
        if (divisor == 1) {
            return String.valueOf(value);
        }
        if (value % divisor == 0) {
            return String.valueOf(value / divisor);
        }
        return String.format((Locale)null, "%.1f", value / (float)divisor);
    }

    private String formatValue(int value) {
        String t = formatNumber(value);
        return suffix == null ? t : t.concat(suffix.length() > 1 ? " "+suffix : suffix);
    }

    // Reads the persisted value so values written directly to SharedPreferences are shown too
    public String getValueText() {
        return formatValue(shouldPersist() ? getPersistedInt(defaultValue) : currentValue);
    }

    @Override
    public void showDialog(Bundle state) {
        UiHelper.showDialog(context, new NavigationDialogBuilder(context)
                .setTitle(getTitle()).setView(onCreateDialogView())
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(android.R.string.ok, (dialog, which) -> {
                    if (callChangeListener(dialogValue)) {
                        currentValue = dialogValue;
                        persistInt(dialogValue);
                        notifyChanged();
                    }
                }).create());
    }
}

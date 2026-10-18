package com.limelight.preferences;

import android.app.Activity;
import android.content.Context;
import android.preference.Preference;
import android.text.TextUtils;
import android.util.AttributeSet;
import android.view.View;
import android.widget.TextView;

import com.google.android.material.chip.Chip;
import com.google.android.material.chip.ChipGroup;
import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.R;
import com.limelight.utils.UiHelper;
import com.limelight.binding.video.DisplayFrameRatePolicy;

public class PresetPreference extends Preference {
    public PresetPreference(Context context, AttributeSet attrs) {
        super(context, attrs);
        setLayoutResource(R.layout.settings_presets);
        setSelectable(true);
    }

    @Override
    protected void onClick() {
        String[] names = getContext().getResources().getStringArray(R.array.stream_preset_names);
        UiHelper.showDialog(getContext(), new NavigationDialogBuilder(getContext())
                .setTitle(R.string.stream_presets)
                .setItems(names, (dialog, which) -> confirmPreset(StreamPreset.values()[which]))
                .setNegativeButton(android.R.string.cancel, null).create());
    }

    private void confirmPreset(StreamPreset preset) {
        String nativeResolution = PreferenceConfiguration.nativeResolution(getContext());
        if (nativeResolution == null) return;
        float refresh = DisplayFrameRatePolicy.maxRefreshRate(((Activity) getContext()).getWindowManager().getDefaultDisplay());
        UiHelper.showDialog(getContext(), new NavigationDialogBuilder(getContext())
                .setTitle(getContext().getResources().getStringArray(R.array.stream_preset_names)[preset.ordinal()])
                .setMessage(R.string.preset_replace_settings)
                .setPositiveButton(android.R.string.ok, (dialog, which) -> {
                    preset.apply(getSharedPreferences(), nativeResolution, refresh);
                    notifyChanged();
                })
                .setNegativeButton(android.R.string.cancel, (dialog, which) -> notifyChanged())
                .setOnCancelListener(dialog -> notifyChanged()).create());
    }

    @Override
    protected void onBindView(View view) {
        super.onBindView(view);
        float refresh = DisplayFrameRatePolicy.maxRefreshRate(((Activity) getContext()).getWindowManager().getDefaultDisplay());
        String nativeResolution = PreferenceConfiguration.nativeResolution(getContext());
        ChipGroup group = view.findViewById(R.id.preset_chips);
        group.removeAllViews();
        String[] names = getContext().getResources().getStringArray(R.array.stream_preset_names);
        String selected = getContext().getString(R.string.settings_custom);
        for (StreamPreset preset : nativeResolution == null ? new StreamPreset[0] : StreamPreset.values()) {
            Chip chip = new Chip(getContext());
            chip.setId(View.generateViewId());
            chip.setText(names[preset.ordinal()]);
            chip.setEllipsize(TextUtils.TruncateAt.END);
            chip.setCheckable(true);
            chip.setEnsureMinTouchTargetSize(true);
            group.addView(chip);
            if (preset.matches(getSharedPreferences(), nativeResolution, refresh)) {
                chip.setChecked(true);
                selected = names[preset.ordinal()];
            }
            chip.setOnClickListener(v -> confirmPreset(preset));
        }
        // A one-line value like every other row: "Native (recommended)" reads as "Native"; the chip keeps the tag.
        ((TextView) view.findViewById(R.id.preset_value)).setText(selected.replaceFirst(" \\(.*\\)$", ""));
        Runnable showHelp = () ->
                UiHelper.showDialog(getContext(), new NavigationDialogBuilder(getContext()).setTitle(R.string.stream_presets)
                        .setMessage(R.string.stream_presets_help).setPositiveButton(android.R.string.ok, null).create());
        view.findViewById(R.id.preset_help).setOnClickListener(v -> showHelp.run());
        view.setTag(R.id.preference_help, showHelp);
    }
}

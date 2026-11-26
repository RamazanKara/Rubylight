package com.limelight.preferences;

import android.text.InputType;
import android.util.DisplayMetrics;
import android.view.View;
import android.widget.CompoundButton;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;

import com.google.android.material.checkbox.MaterialCheckBox;
import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.google.android.material.slider.Slider;
import com.google.android.material.textfield.TextInputEditText;
import com.google.android.material.textfield.TextInputLayout;
import com.limelight.R;
import com.limelight.binding.video.DisplayFrameRatePolicy;
import com.limelight.binding.video.PyroWaveDecoderRenderer;
import com.limelight.binding.video.UpscalingPolicy;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.wol.WakeTargets;
import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.ui.StepSlider;
import com.limelight.utils.UiHelper;

import java.util.List;

import static com.limelight.preferences.HostStreamProfileSummary.*;

/**
 * Interactive picker and form dialogs for per-PC, named-profile, and per-game overrides in {@link HostStreamSettings}.
 */
final class HostStreamSettingDialogs {
    private HostStreamSettingDialogs() {}

    static void showSwitch(HostStreamSettings.SettingsFragment fragment, HostStreamSettings activity,
                           String key, int title) {
        boolean allPcs = switchValue(activity.global, key);
        Boolean own = switchOverride(activity, key);
        CharSequence[] items = {
                sameAs(activity, switchText(activity, key, allPcs)),
                switchText(activity, key, true),
                switchText(activity, key, false)};
        int checked = own == null ? 0 : own ? 1 : 2;
        new NavigationDialogBuilder(activity).setTitle(title)
                .setSingleChoiceItems(items, checked, (dialog, which) -> {
                    setSwitchOverride(activity, key, which == 0 ? null : which == 1);
                    fragment.updateSummaries();
                    dialog.dismiss();
                }).setNegativeButton(android.R.string.cancel, null).show();
    }

    static void showVrr(HostStreamSettings.SettingsFragment fragment, HostStreamSettings activity) {
        HostStreamProfile own = activity.overrides;
        String[] names = activity.getResources().getStringArray(R.array.vrr_choice_names);
        CharSequence[] items = {sameAs(activity, vrrText(activity, activity.global)),
                names[VrrChoice.OFF], names[VrrChoice.ON], names[VrrChoice.FASTEST]};
        int checked = own.vrr == null ? 0 : 1 + VrrChoice.of(own.vrr,
                own.realVrr != null ? own.realVrr : activity.effective().realVrr);
        new NavigationDialogBuilder(activity).setTitle(R.string.title_vrr)
                .setSingleChoiceItems(items, checked, (dialog, which) -> {
                    VrrChoice.save(own, which == 0 ? null : which - 1);
                    fragment.updateSummaries();
                    dialog.dismiss();
                }).setNegativeButton(android.R.string.cancel, null).show();
    }

    static void showPreset(HostStreamSettings.SettingsFragment fragment, HostStreamSettings activity) {
        GameProfilePreset[] presets = GameProfilePreset.values();
        int quality = activity.qualityBitrate();
        CharSequence[] items = new CharSequence[presets.length];
        for (int i = 0; i < presets.length; i++) {
            items[i] = activity.getString(presets[i].title) + "\n" + activity.getString(presets[i].summary);
        }
        new NavigationDialogBuilder(activity).setTitle(R.string.game_preset_title)
                .setSingleChoiceItems(items, GameProfilePreset.of(activity.overrides, quality).ordinal(),
                        (dialog, which) -> {
                            presets[which].apply(activity.overrides, quality);
                            fragment.updateSummaries();
                            dialog.dismiss();
                        }).setNegativeButton(android.R.string.cancel, null).show();
    }

    static void showPacing(HostStreamSettings.SettingsFragment fragment, HostStreamSettings activity) {
        String[] names = activity.getResources().getStringArray(R.array.frame_pacing_choice_names);
        CharSequence[] items = new CharSequence[names.length + 1];
        items[0] = sameAs(activity, pacingText(activity, activity.global));
        System.arraycopy(names, 0, items, 1, names.length);
        HostStreamProfile own = activity.overrides;
        PreferenceConfiguration main = own.pacing == null ? null : PreferenceConfiguration.readPreferences(activity);
        int checked = own.pacing == null ? 0 : 1 + Math.max(0, FramePacingChoice.index(FramePacingChoice.of(own.pacing,
                own.dropLateFrames != null ? own.dropLateFrames : main.dropLateFrames,
                own.hostFrameTimes != null ? own.hostFrameTimes : main.gridHostFrameTimes)));
        new NavigationDialogBuilder(activity).setTitle(R.string.title_frame_pacing)
                .setSingleChoiceItems(items, checked, (dialog, which) -> {
                    FramePacingChoice.save(activity.overrides, which == 0 ? null : FramePacingChoice.VALUES[which - 1]);
                    fragment.updateSummaries();
                    dialog.dismiss();
                }).setNegativeButton(android.R.string.cancel, null).show();
    }

    static void showEnhancement(HostStreamSettings.SettingsFragment fragment, HostStreamSettings activity,
                                HostStreamProfile.Field field, int title) {
        Boolean own = field == HostStreamProfile.Field.RTX_HDR ? activity.overrides.rtxHdr : activity.overrides.rtxVideo;
        Boolean below = field == HostStreamProfile.Field.RTX_HDR ? activity.global.rtxHdr : activity.global.rtxVideo;
        CharSequence first = activity.mode == HostStreamSettings.Mode.GAME
                ? sameAs(activity, enhancementText(activity, below))
                : enhancementText(activity, null);
        CharSequence[] items = {first, onOff(activity, true), onOff(activity, false)};
        int checked = own == null ? 0 : own ? 1 : 2;
        new NavigationDialogBuilder(activity).setTitle(title)
                .setSingleChoiceItems(items, checked, (dialog, which) -> {
                    Boolean chosen = which == 0 ? null : which == 1;
                    if (field == HostStreamProfile.Field.RTX_HDR) activity.overrides.rtxHdr = chosen;
                    else activity.overrides.rtxVideo = chosen;
                    fragment.updateSummaries();
                    dialog.dismiss();
                }).setNegativeButton(android.R.string.cancel, null).show();
    }

    static void showHdrFormat(HostStreamSettings.SettingsFragment fragment, HostStreamSettings activity) {
        List<HdrFormatChoice> values = HdrFormatChoice.offered(activity, activity.overrides.hdrFormat);
        String[] names = activity.getResources().getStringArray(R.array.hdr_format_names);
        CharSequence[] items = new CharSequence[values.size() + 1];
        items[0] = sameAs(activity, names[activity.global.hdrFormat.ordinal()]);
        for (int i = 0; i < values.size(); i++) items[i + 1] = names[values.get(i).ordinal()];
        HdrFormatChoice own = activity.overrides.hdrFormat;
        int checked = own == null ? 0 : values.indexOf(own) < 0 ? -1 : values.indexOf(own) + 1;
        new NavigationDialogBuilder(activity).setTitle(R.string.title_hdr_format)
                .setSingleChoiceItems(items, checked, (dialog, which) -> {
                    activity.overrides.hdrFormat = which == 0 ? null : values.get(which - 1);
                    fragment.updateSummaries();
                    dialog.dismiss();
                }).setNegativeButton(android.R.string.cancel, null).show();
    }

    static void showDisplayScale(HostStreamSettings.SettingsFragment fragment, HostStreamSettings activity) {
        int[] values = PreferenceConfiguration.VIRTUAL_DISPLAY_DPI_VALUES;
        CharSequence[] items = new CharSequence[values.length + 1];
        items[0] = sameAs(activity, displayScaleText(activity, activity.global.virtualDisplayDpi));
        int checked = 0;
        for (int i = 0; i < values.length; i++) {
            items[i + 1] = displayScaleText(activity, values[i]);
            if (activity.overrides.displayScale != null && activity.overrides.displayScale == values[i]) checked = i + 1;
        }
        new NavigationDialogBuilder(activity).setTitle(R.string.title_virtual_display_dpi)
                .setSingleChoiceItems(items, checked, (dialog, which) -> {
                    activity.overrides.displayScale = which == 0 ? null : values[which - 1];
                    fragment.updateSummaries();
                    dialog.dismiss();
                }).setNegativeButton(android.R.string.cancel, null).show();
    }

    static void showCodec(HostStreamSettings.SettingsFragment fragment, HostStreamSettings activity) {
        String[] codecs = activity.getResources().getStringArray(R.array.host_profile_codecs);
        int offered = gate(activity, HostCapabilities.Feature.PYROWAVE).isVisible() ? codecs.length :
                PreferenceConfiguration.FormatOption.FORCE_PYROWAVE.ordinal();
        CharSequence[] items = new CharSequence[offered + 1];
        items[0] = sameAs(activity, codecs[activity.global.videoFormat.ordinal()]);
        System.arraycopy(codecs, 0, items, 1, offered);
        int noPyroWave = PyroWaveDecoderRenderer.neverAvailableReason();
        int pyroWave = PreferenceConfiguration.FormatOption.FORCE_PYROWAVE.ordinal() + 1;
        if (noPyroWave != 0 && pyroWave < items.length) {
            items[pyroWave] = items[pyroWave] + " · " + activity.getString(noPyroWave);
        }
        PreferenceConfiguration.FormatOption own = activity.overrides.codec;
        int checked = own == null ? 0 : own.ordinal() < offered ? own.ordinal() + 1 : -1;
        new NavigationDialogBuilder(activity).setTitle(R.string.title_video_format)
                .setSingleChoiceItems(items, checked, (dialog, which) -> {
                    activity.overrides.codec = which == 0 ? null :
                            PreferenceConfiguration.FormatOption.values()[which - 1];
                    fragment.updateSummaries();
                    dialog.dismiss();
                }).setNegativeButton(android.R.string.cancel, null).show();
    }

    static void showUpscaling(HostStreamSettings.SettingsFragment fragment, HostStreamSettings activity) {
        String[] modes = activity.getResources().getStringArray(R.array.upscaling_names);
        CharSequence[] items = new CharSequence[modes.length + 1];
        items[0] = sameAs(activity, modes[activity.global.upscalingMode.ordinal()]);
        System.arraycopy(modes, 0, items, 1, modes.length);
        UpscalingPolicy.Mode own = activity.overrides.upscalingMode;
        AlertDialog picker = new NavigationDialogBuilder(activity).setTitle(R.string.title_upscaling)
                .setSingleChoiceItems(items, own == null ? 0 : own.ordinal() + 1, (dialog, which) -> {
                    activity.overrides.upscalingMode = which == 0 ? null : UpscalingPolicy.Mode.values()[which - 1];
                    fragment.updateSummaries();
                    dialog.dismiss();
                }).setNeutralButton(R.string.help, null)
                .setNegativeButton(android.R.string.cancel, null).create();
        UiHelper.showDialog(activity, picker);
        picker.getButton(AlertDialog.BUTTON_NEUTRAL).setOnClickListener(view ->
                UiHelper.showDialog(activity, new NavigationDialogBuilder(activity).setTitle(R.string.title_upscaling)
                        .setMessage(R.string.summary_upscaling).setPositiveButton(android.R.string.ok, null).create()));
    }

    static void showDisplay(HostStreamSettings.SettingsFragment fragment, HostStreamSettings activity) {
        PreferenceConfiguration config = activity.effective();
        CharSequence allPcs = DisplayArrangementChoice.names(activity, true)[DisplayArrangementChoice.choice(
                HostDisplayChoice.loadMain(activity), switchValue(activity.global, "virtual"), true)];
        AlertDialog picker = HostDisplaySettings.create(activity, activity.hostUuid, activity.computer.displayName(),
                sameAs(activity, allPcs.toString()), ownDisplay(activity), switchValue(config, "virtual"),
                activity::saveVirtualDisplay, fragment::updateSummaries);
        fragment.showEditor("display", picker);
        HostDisplaySettings.bindHelp(activity, picker);
    }

    static void showSharpness(HostStreamSettings.SettingsFragment fragment, HostStreamSettings activity) {
        PreferenceConfiguration config = activity.effective();
        LinearLayout form = form(activity);
        TextView value = new TextView(activity);
        Slider slider = new Slider(activity);
        slider.setId(R.id.host_profile_sharpness);
        MaterialCheckBox allPcs = sameAsAll(activity, form, activity.getString(R.string.upscaling_sharpness_value,
                activity.global.upscalingSharpness), activity.overrides.upscalingSharpness == null, slider);
        value.setText(activity.getString(R.string.upscaling_sharpness_value, config.upscalingSharpness));
        value.setTextAppearance(activity, R.style.TextAppearance_Nightjar_ScreenTitle);
        form.addView(value);
        slider.setContentDescription(activity.getString(R.string.title_upscaling_sharpness));
        slider.setValueFrom(0);
        slider.setValueTo(100);
        slider.setStepSize(1);
        slider.setValue(config.upscalingSharpness);
        StepSlider.showValue(slider, strength -> activity.getString(R.string.upscaling_sharpness_value, strength));
        slider.setEnabled(!allPcs.isChecked());
        slider.addOnChangeListener((view, strength, fromUser) ->
                value.setText(activity.getString(R.string.upscaling_sharpness_value, Math.round(strength))));
        form.addView(slider);
        fragment.showEditor("sharpness", new NavigationDialogBuilder(activity)
                .setTitle(R.string.title_upscaling_sharpness).setView(scrollable(activity, form))
                .setPositiveButton(android.R.string.ok, (dialog, which) -> {
                    activity.overrides.upscalingSharpness = allPcs.isChecked() ? null : Math.round(slider.getValue());
                    fragment.updateSummaries();
                }).setNegativeButton(android.R.string.cancel, null).create());
    }

    static void showResolution(HostStreamSettings.SettingsFragment fragment, HostStreamSettings activity) {
        PreferenceConfiguration config = activity.effective();
        HostStreamProfile own = activity.overrides;
        LinearLayout form = form(activity);
        MaterialCheckBox nativeChoice = new MaterialCheckBox(activity);
        nativeChoice.setId(R.id.host_profile_follow_screen);
        nativeChoice.setText(R.string.host_profile_native);
        nativeChoice.setChecked(config.resolutionFollowsScreen);
        EditText[] size = new EditText[2];
        MaterialCheckBox allPcs = sameAsAll(activity, form, resolutionText(activity, activity.global),
                own.nativeResolution == null, nativeChoice);
        form.addView(nativeChoice);
        size[0] = field(activity, form, R.string.host_profile_width, Integer.toString(config.width), false);
        size[1] = field(activity, form, R.string.host_profile_height, Integer.toString(config.height), false);
        Runnable refresh = () -> {
            nativeChoice.setEnabled(!allPcs.isChecked());
            enable(!allPcs.isChecked() && !nativeChoice.isChecked(), size);
        };
        allPcs.setOnCheckedChangeListener((button, checked) -> refresh.run());
        nativeChoice.setOnCheckedChangeListener((button, checked) -> refresh.run());
        refresh.run();
        AlertDialog dialog = new NavigationDialogBuilder(activity).setTitle(R.string.title_resolution_list)
                .setView(scrollable(activity, form)).setPositiveButton(android.R.string.ok, null)
                .setNegativeButton(android.R.string.cancel, null)
                .setNeutralButton(R.string.host_profile_match_screen, null).create();
        dialog.setOnShowListener(ignored -> {
            dialog.getButton(AlertDialog.BUTTON_NEUTRAL).setOnClickListener(v -> {
                allPcs.setChecked(false);
                nativeChoice.setChecked(false);
                String panel = PreferenceConfiguration.nativeResolution(activity);
                if (panel == null) {
                    DisplayMetrics metrics = new DisplayMetrics();
                    activity.getWindowManager().getDefaultDisplay().getRealMetrics(metrics);
                    panel = Math.max(metrics.widthPixels, metrics.heightPixels) + "x" +
                            Math.min(metrics.widthPixels, metrics.heightPixels);
                }
                size[0].setText(Integer.toString(PreferenceConfiguration.getWidthFromResolutionString(panel)));
                size[1].setText(Integer.toString(PreferenceConfiguration.getHeightFromResolutionString(panel)));
            });
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(v -> {
                if (allPcs.isChecked() || nativeChoice.isChecked()) {
                    own.nativeResolution = allPcs.isChecked() ? null : Boolean.TRUE;
                    own.width = own.height = null;
                    fragment.updateSummaries();
                    dialog.dismiss();
                    return;
                }
                Integer w = readNumber(activity, size[0], 64, 16384);
                Integer h = readNumber(activity, size[1], 64, 16384);
                if (w != null && h != null) {
                    own.nativeResolution = false;
                    own.width = w;
                    own.height = h;
                    fragment.updateSummaries();
                    dialog.dismiss();
                }
            });
        });
        fragment.showEditor("resolution", dialog);
    }

    static void showNumber(HostStreamSettings.SettingsFragment fragment, HostStreamSettings activity,
                           String key, int title) {
        PreferenceConfiguration config = activity.effective();
        PreferenceConfiguration global = activity.global;
        HostStreamProfile own = activity.overrides;
        LinearLayout form = form(activity);
        String value, allPcsValue;
        boolean followsAll;
        if (key.equals("refresh")) {
            value = HostStreamProfile.formatRefreshRate(config.launchRefreshRateX100 == 0 ?
                    config.fps * 100 : config.launchRefreshRateX100);
            allPcsValue = refreshText(activity, global);
            followsAll = own.refreshRateX100 == null;
        } else if (key.equals("bitrate")) {
            value = HostStreamProfile.formatBitrateMbps(config.bitrate);
            allPcsValue = HostStreamProfile.formatBitrateMbps(global.bitrate) + " Mbps";
            followsAll = own.bitrate == null;
        } else {
            value = Integer.toString(config.virtualDisplayScale);
            allPcsValue = global.virtualDisplayScale + "%";
            followsAll = own.renderScale == null;
        }
        EditText[] field = new EditText[1];
        MaterialCheckBox allPcs = sameAsAll(activity, form, allPcsValue, followsAll);
        field[0] = field(activity, form, title, value, !key.equals("scale"));
        allPcs.setOnCheckedChangeListener((button, checked) -> field[0].setEnabled(!checked));
        field[0].setEnabled(!followsAll);
        MaterialAlertDialogBuilder builder = new NavigationDialogBuilder(activity).setTitle(title)
                .setView(scrollable(activity, form))
                .setPositiveButton(android.R.string.ok, null).setNegativeButton(android.R.string.cancel, null);
        if (key.equals("refresh")) builder.setNeutralButton(R.string.host_profile_match_screen, null);
        AlertDialog dialog = builder.create();
        dialog.setOnShowListener(ignored -> {
            if (key.equals("refresh")) {
                dialog.getButton(AlertDialog.BUTTON_NEUTRAL).setOnClickListener(v -> {
                    allPcs.setChecked(false);
                    field[0].setText(HostStreamProfile.formatRefreshRate(Math.round(100 *
                            DisplayFrameRatePolicy.maxRefreshRate(activity.getWindowManager().getDefaultDisplay()))));
                });
            }
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(v -> {
                showError(field[0], null);
                boolean clear = allPcs.isChecked();
                try {
                    if (key.equals("refresh")) {
                        own.refreshRateX100 = clear ? null : HostStreamProfile.parseRefreshRate(field[0].getText().toString());
                    } else if (key.equals("bitrate")) {
                        own.bitrate = clear ? null : HostStreamProfile.parseBitrateMbps(field[0].getText().toString());
                    } else if (clear) {
                        own.renderScale = null;
                    } else {
                        Integer scale = readNumber(activity, field[0], 50, 200);
                        if (scale == null) return;
                        own.renderScale = scale;
                    }
                    fragment.updateSummaries();
                    dialog.dismiss();
                } catch (IllegalArgumentException e) {
                    showError(field[0], activity.getString(key.equals("refresh") ?
                            R.string.host_profile_invalid_refresh : R.string.host_profile_invalid_bitrate));
                }
            });
        });
        fragment.showEditor(key, dialog);
    }

    static void showWakeMacs(HostStreamSettings.SettingsFragment fragment, HostStreamSettings activity) {
        WakeTargets targets = new WakeTargets(activity);
        String uuid = activity.computer.uuid;
        LinearLayout form = form(activity);
        TextView help = new TextView(activity);
        help.setText(R.string.pc_settings_wake_macs_help);
        help.setTextAppearance(activity, R.style.TextAppearance_Nightjar_Body);
        help.setTextColor(com.google.android.material.color.MaterialColors.getColor(help,
                com.google.android.material.R.attr.colorOnSurfaceVariant));
        form.addView(help);
        TextInputLayout container = new TextInputLayout(activity);
        container.setHint(R.string.pc_settings_wake_macs);
        EditText field = new TextInputEditText(container.getContext());
        field.setId(R.id.host_profile_field_primary);
        field.setContentDescription(activity.getString(R.string.pc_settings_wake_macs));
        field.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
                | InputType.TYPE_TEXT_FLAG_MULTI_LINE);
        field.setMinHeight(Math.round(56 * activity.getResources().getDisplayMetrics().density));
        field.setText(android.text.TextUtils.join("\n", targets.manual(uuid)));
        container.addView(field);
        form.addView(container);
        AlertDialog dialog = new NavigationDialogBuilder(activity).setTitle(R.string.pc_settings_wake_macs)
                .setView(scrollable(activity, form)).setPositiveButton(android.R.string.ok, null)
                .setNegativeButton(android.R.string.cancel, null).create();
        dialog.setOnShowListener(ignored -> dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(v -> {
            List<String> macs = WakeTargets.parseMacs(field.getText().toString());
            if (macs == null) {
                showError(field, activity.getString(R.string.pc_settings_wake_macs_invalid));
                return;
            }
            targets.storeManual(uuid, macs);
            fragment.showActions();
            dialog.dismiss();
        }));
        fragment.showEditor("wake_macs", dialog);
    }

    private static LinearLayout form(HostStreamSettings activity) {
        LinearLayout form = new LinearLayout(activity);
        form.setOrientation(LinearLayout.VERTICAL);
        int padding = Math.round(24 * activity.getResources().getDisplayMetrics().density);
        form.setPadding(padding, padding / 3, padding, 0);
        return form;
    }

    private static ScrollView scrollable(HostStreamSettings activity, LinearLayout form) {
        ScrollView scroll = new ScrollView(activity);
        scroll.setFillViewport(true);
        scroll.setClipToPadding(false);
        scroll.addView(form);
        return scroll;
    }

    private static MaterialCheckBox sameAsAll(HostStreamSettings activity, LinearLayout form,
                                              String allPcsValue, boolean checked, View... fields) {
        MaterialCheckBox box = new MaterialCheckBox(activity);
        box.setId(R.id.host_profile_follow_all);
        box.setText(sameAs(activity, allPcsValue));
        box.setChecked(checked);
        form.addView(box);
        CompoundButton.OnCheckedChangeListener listener = (button, isChecked) -> {
            for (View field : fields) field.setEnabled(!isChecked);
        };
        box.setOnCheckedChangeListener(listener);
        return box;
    }

    private static void enable(boolean enabled, View... views) {
        for (View view : views) view.setEnabled(enabled);
    }

    private static EditText field(HostStreamSettings activity, LinearLayout form,
                                  int title, String value, boolean decimal) {
        TextInputLayout container = new TextInputLayout(activity);
        container.setHint(title);
        EditText field = new TextInputEditText(container.getContext());
        field.setId(form.findViewById(R.id.host_profile_field_primary) == null ?
                R.id.host_profile_field_primary : R.id.host_profile_field_secondary);
        field.setContentDescription(activity.getString(title));
        field.setInputType(InputType.TYPE_CLASS_NUMBER | (decimal ? InputType.TYPE_NUMBER_FLAG_DECIMAL : 0));
        field.setSingleLine(true);
        field.setSelectAllOnFocus(true);
        field.setMinHeight(Math.round(56 * activity.getResources().getDisplayMetrics().density));
        field.setText(value);
        container.addView(field);
        form.addView(container);
        return field;
    }

    private static void showError(EditText field, String error) {
        View parent = (View) field.getParent();
        while (!(parent instanceof TextInputLayout)) parent = (View) parent.getParent();
        ((TextInputLayout) parent).setError(error);
    }

    private static Integer readNumber(HostStreamSettings activity, EditText field, int min, int max) {
        showError(field, null);
        try {
            return HostStreamProfile.requireRange(Integer.parseInt(field.getText().toString().trim()), min, max);
        } catch (IllegalArgumentException e) {
            showError(field, activity.getString(R.string.host_profile_invalid_number, min, max));
            return null;
        }
    }
}

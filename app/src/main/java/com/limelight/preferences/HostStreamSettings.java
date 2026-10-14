package com.limelight.preferences;

import android.app.Activity;
import android.content.Intent;
import android.os.Bundle;
import android.preference.Preference;
import android.preference.PreferenceCategory;
import android.preference.PreferenceFragment;
import android.preference.PreferenceScreen;
import android.text.InputType;
import android.util.DisplayMetrics;
import android.view.LayoutInflater;
import android.view.View;
import android.view.ViewGroup;
import android.widget.CompoundButton;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.ListView;
import android.widget.Toast;
import android.widget.TextView;
import android.widget.ScrollView;

import androidx.appcompat.app.AlertDialog;
import androidx.appcompat.app.AppCompatActivity;
import androidx.activity.OnBackPressedCallback;
import com.google.android.material.appbar.MaterialToolbar;
import com.google.android.material.checkbox.MaterialCheckBox;
import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.google.android.material.textfield.TextInputEditText;
import com.google.android.material.textfield.TextInputLayout;
import com.limelight.R;
import com.limelight.binding.video.UpscalingPolicy;
import com.google.android.material.slider.Slider;
import com.limelight.utils.UiHelper;

/**
 * One PC's stream settings. Each value is either this PC's own or "Same as all PCs", which
 * follows the main Stream settings, so a global change reaches every PC that doesn't set it.
 */
public final class HostStreamSettings extends AppCompatActivity {
    // The settings for all PCs, and this PC's own values on top of them.
    private PreferenceConfiguration global;
    private HostStreamProfile overrides;
    private String hostUuid;
    private String savedProfile;
    private String editorKey;
    private AlertDialog editorDialog;
    private Bundle restoredEditor;
    private String restoredEditorKey;

    public static void show(Activity activity, String hostUuid, String hostName) {
        activity.startActivity(new Intent(activity, HostStreamSettings.class)
                .putExtra("host_uuid", hostUuid).putExtra("host_name", hostName));
    }

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        UiHelper.setLocale(this);
        hostUuid = getIntent().getStringExtra("host_uuid");
        if (hostUuid == null) {
            finish();
            return;
        }
        global = PreferenceConfiguration.readPreferences(this);
        PreferenceConfiguration.migratePcProfilesToOverrides(this);
        HostStreamProfile saved = HostStreamProfile.load(this, hostUuid);
        overrides = saved == null ? new HostStreamProfile() : saved;
        savedProfile = overrides.serialize();
        if (state != null && state.getString("draft") != null) {
            try {
                overrides = HostStreamProfile.deserialize(state.getString("draft"));
            } catch (IllegalArgumentException ignored) {
                // Keep the saved values.
            }
            restoredEditorKey = state.getString("editor_key");
            restoredEditor = state.getBundle("editor_state");
        }
        setContentView(R.layout.activity_host_stream_settings);
        MaterialToolbar toolbar = findViewById(R.id.host_profile_toolbar);
        ((TextView) findViewById(R.id.host_profile_name)).setText(getIntent().getStringExtra("host_name"));
        toolbar.setNavigationOnClickListener(v -> leaveSettings());
        getOnBackPressedDispatcher().addCallback(this, new OnBackPressedCallback(true) {
            @Override
            public void handleOnBackPressed() {
                leaveSettings();
            }
        });
        toolbar.inflateMenu(R.menu.host_profile);
        toolbar.setOnMenuItemClickListener(item -> {
            new MaterialAlertDialogBuilder(this).setTitle(R.string.host_profile_menu)
                    .setMessage(R.string.host_profile_explanation).setPositiveButton(android.R.string.ok, null).show();
            return true;
        });
        findViewById(R.id.host_profile_save).setOnClickListener(v -> {
            overrides.save(this, hostUuid);
            Toast.makeText(this, overrides.isEmpty() ? R.string.host_profile_reset_done : R.string.host_profile_saved,
                    Toast.LENGTH_SHORT).show();
            finish();
        });
        updateSaveState();
        if (state == null) {
            getFragmentManager().beginTransaction().replace(R.id.host_profile_settings, new SettingsFragment()).commit();
        }
        UiHelper.notifyNewRootView(this);
    }

    private void leaveSettings() {
        if (savedProfile.equals(overrides.serialize())) {
            finish();
            return;
        }
        UiHelper.showDialog(this, new MaterialAlertDialogBuilder(this).setTitle(R.string.host_profile_unsaved_title)
                .setMessage(R.string.host_profile_unsaved_message)
                .setPositiveButton(R.string.host_profile_save, (dialog, which) -> {
                    overrides.save(this, hostUuid);
                    finish();
                })
                .setNegativeButton(R.string.host_profile_discard, (dialog, which) -> finish())
                .setNeutralButton(android.R.string.cancel, null).create());
    }

    private void updateSaveState() {
        boolean changed = !savedProfile.equals(overrides.serialize());
        findViewById(R.id.host_profile_save).setEnabled(changed);
        ((TextView) findViewById(R.id.host_profile_status)).setText(changed ?
                R.string.settings_profile_unsaved_context : R.string.settings_profile_saved_context);
    }

    /** What the next stream from this PC uses: the global settings with this PC's values on top. */
    private PreferenceConfiguration effective() {
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(this);
        boolean followedScreen = config.resolutionFollowsScreen;
        overrides.applyTo(config);
        if (config.resolutionFollowsScreen && !followedScreen) {
            String nativeResolution = PreferenceConfiguration.nativeResolution(this);
            if (nativeResolution != null) {
                config.width = PreferenceConfiguration.getWidthFromResolutionString(nativeResolution);
                config.height = PreferenceConfiguration.getHeightFromResolutionString(nativeResolution);
            }
        }
        return config;
    }

    @Override
    protected void onSaveInstanceState(Bundle state) {
        super.onSaveInstanceState(state);
        if (overrides != null) state.putString("draft", overrides.serialize());
        if (editorDialog != null && editorDialog.isShowing()) {
            state.putString("editor_key", editorKey);
            state.putBundle("editor_state", editorDialog.onSaveInstanceState());
        }
    }

    public static class SettingsFragment extends PreferenceFragment {
        private HostStreamSettings activity;
        private PreferenceCategory stream;

        @Override
        public void onActivityCreated(Bundle state) {
            super.onActivityCreated(state);
            activity = (HostStreamSettings) getActivity();
            PreferenceScreen screen = getPreferenceManager().createPreferenceScreen(activity);
            setPreferenceScreen(screen);
            // Saved separately, so the display choice also applies when profile edits are discarded.
            PreferenceCategory displayCategory = new PreferenceCategory(activity);
            displayCategory.setTitle(R.string.host_display_menu);
            displayCategory.setLayoutResource(R.layout.settings_category);
            screen.addPreference(displayCategory);
            Preference display = new Preference(activity);
            display.setKey("host_display");
            display.setTitle(R.string.host_display_menu);
            display.setLayoutResource(R.layout.settings_preference);
            display.setPersistent(false);
            displayCategory.addPreference(display);
            display.setOnPreferenceClickListener(pref -> {
                showDisplay();
                return true;
            });
            stream = displayCategory;
            row("virtual", R.string.title_virtual_display, () -> showSwitch("virtual", R.string.title_virtual_display));
            row("scale", R.string.title_virtual_display_scale, () -> showNumber("scale", R.string.host_profile_scale));
            stream = new PreferenceCategory(activity);
            stream.setTitle(R.string.settings_section_video);
            stream.setLayoutResource(R.layout.settings_category);
            screen.addPreference(stream);
            row("resolution", R.string.title_resolution_list, this::showResolution);
            row("refresh", R.string.title_fps_list, () -> showNumber("refresh", R.string.host_profile_refresh));
            row("bitrate", R.string.title_seekbar_bitrate, () -> showNumber("bitrate", R.string.host_profile_bitrate));
            row("codec", R.string.title_video_format, this::showCodec);
            row("upscaling", R.string.title_upscaling, this::showUpscaling);
            row("sharpness", R.string.title_upscaling_sharpness, this::showSharpness);
            row("hdr", R.string.title_enable_hdr, () -> showSwitch("hdr", R.string.title_enable_hdr));
            row("vrr", R.string.title_vrr, () -> showSwitch("vrr", R.string.title_vrr));
            row("yuv444", R.string.title_yuv444, () -> showSwitch("yuv444", R.string.title_yuv444));
            row("full_range", R.string.title_full_range, () -> showSwitch("full_range", R.string.title_full_range));
            row("reset", R.string.host_profile_reset, () -> UiHelper.showDialog(activity,
                    new MaterialAlertDialogBuilder(activity).setTitle(R.string.host_profile_reset)
                            .setMessage(R.string.settings_profile_reset_message)
                            .setPositiveButton(R.string.host_profile_reset, (dialog, which) -> {
                                activity.overrides = new HostStreamProfile();
                                updateSummaries();
                            }).setNegativeButton(android.R.string.cancel, null).create()));
            Preference footer = new Preference(activity);
            footer.setLayoutResource(R.layout.settings_preference);
            footer.setSummary(R.string.host_profile_footer);
            footer.setSelectable(false);
            screen.addPreference(footer);
            updateSummaries();
        }

        private void row(String key, int title, Runnable onClick) {
            Preference preference = new Preference(activity);
            preference.setKey(key);
            preference.setTitle(title);
            preference.setLayoutResource(R.layout.settings_preference);
            preference.setPersistent(false);
            preference.setOnPreferenceClickListener(pref -> {
                onClick.run();
                return true;
            });
            stream.addPreference(preference);
        }

        private String onOff(boolean value) {
            return getString(value ? R.string.host_profile_on : R.string.host_profile_off);
        }

        /** The value, marked when it comes from the settings for all PCs. */
        private String summary(boolean own, String value) {
            return own ? value : getString(R.string.host_profile_follows, value);
        }

        private String resolutionText(PreferenceConfiguration config) {
            return config.resolutionFollowsScreen ? getString(R.string.host_profile_native) :
                    config.width + " × " + config.height;
        }

        private String refreshText(PreferenceConfiguration config) {
            return HostStreamProfile.formatRefreshRate(config.launchRefreshRateX100 == 0 ?
                    config.fps * 100 : config.launchRefreshRateX100) + " " + getString(R.string.fps_suffix_fps);
        }

        private String upscalingText(PreferenceConfiguration config) {
            String upscaling = getResources().getStringArray(R.array.upscaling_names)[config.upscalingMode.ordinal()];
            if (config.upscalingMode != UpscalingPolicy.Mode.OFF) {
                if (config.enableHdr) upscaling += "\n" + getString(R.string.upscaling_hdr);
                else if (config.videoFormat == PreferenceConfiguration.FormatOption.FORCE_PYROWAVE) {
                    upscaling += "\n" + getString(R.string.upscaling_pyrowave);
                }
            }
            return upscaling;
        }

        private boolean switchValue(PreferenceConfiguration config, String key) {
            switch (key) {
                case "hdr": return config.enableHdr;
                case "vrr": return config.vrr;
                case "virtual": return config.virtualDisplay;
                case "yuv444": return config.enableYuv444;
                default: return config.fullRange;
            }
        }

        private Boolean switchOverride(String key) {
            HostStreamProfile own = activity.overrides;
            switch (key) {
                case "hdr": return own.hdr;
                case "vrr": return own.vrr;
                case "virtual": return own.virtualDisplay;
                case "yuv444": return own.yuv444;
                default: return own.fullRange;
            }
        }

        private void setSwitchOverride(String key, Boolean value) {
            HostStreamProfile own = activity.overrides;
            switch (key) {
                case "hdr": own.hdr = value; break;
                case "vrr": own.vrr = value; break;
                case "virtual": own.virtualDisplay = value; break;
                case "yuv444": own.yuv444 = value; break;
                default: own.fullRange = value; break;
            }
        }

        private void updateSummaries() {
            PreferenceConfiguration config = activity.effective();
            HostStreamProfile own = activity.overrides;
            findPreference("resolution").setSummary(summary(own.nativeResolution != null, resolutionText(config)) +
                    "\n" + getString(R.string.summary_resolution_list));
            findPreference("refresh").setSummary(summary(own.refreshRateX100 != null, refreshText(config)));
            findPreference("bitrate").setSummary(summary(own.bitrate != null,
                    HostStreamProfile.formatBitrateMbps(config.bitrate) + " Mbps"));
            findPreference("codec").setSummary(summary(own.codec != null,
                    getResources().getStringArray(R.array.host_profile_codecs)[config.videoFormat.ordinal()]));
            findPreference("upscaling").setSummary(summary(own.upscalingMode != null, upscalingText(config)));
            findPreference("sharpness").setSummary(summary(own.upscalingSharpness != null,
                    getString(R.string.upscaling_sharpness_value, config.upscalingSharpness)));
            findPreference("sharpness").setEnabled(config.upscalingMode == UpscalingPolicy.Mode.FSR1 ||
                    config.upscalingMode == UpscalingPolicy.Mode.SGSR1);
            for (String key : new String[] {"hdr", "vrr", "virtual", "yuv444", "full_range"}) {
                findPreference(key).setSummary(summary(switchOverride(key) != null, onOff(switchValue(config, key))));
            }
            findPreference("vrr").setSummary(findPreference("vrr").getSummary() +
                    "\n" + getString(R.string.summary_vrr));
            findPreference("scale").setSummary(summary(own.renderScale != null, config.virtualDisplayScale + "%") +
                    "\n" + getString(R.string.summary_virtual_display_scale));
            findPreference("reset").setEnabled(!own.isEmpty());
            findPreference("reset").setSummary(own.isEmpty() ? getString(R.string.host_profile_status_global) :
                    getString(R.string.host_profile_status_custom));
            Preference display = findPreference("host_display");
            HostDisplayChoice displayChoice = HostDisplayChoice.load(activity, activity.hostUuid);
            findPreference("virtual").setEnabled(displayChoice == HostDisplayChoice.HOST_DEFAULT);
            if (displayChoice != HostDisplayChoice.HOST_DEFAULT) {
                findPreference("virtual").setSummary(getString(R.string.host_display_controls_virtual,
                        getString(displayChoice.label)));
            }
            if (display != null) {
                display.setSummary(getString(displayChoice.label) + "\n" + getString(R.string.settings_profile_display_immediate));
            }
            activity.updateSaveState();
        }

        private void showSwitch(String key, int title) {
            boolean allPcs = switchValue(activity.global, key);
            Boolean own = switchOverride(key);
            CharSequence[] items = {
                    getString(R.string.host_profile_same_as_all_value, onOff(allPcs)),
                    getString(R.string.host_profile_on),
                    getString(R.string.host_profile_off)};
            int checked = own == null ? 0 : own ? 1 : 2;
            new MaterialAlertDialogBuilder(activity).setTitle(title)
                    .setSingleChoiceItems(items, checked, (dialog, which) -> {
                        setSwitchOverride(key, which == 0 ? null : which == 1);
                        updateSummaries();
                        dialog.dismiss();
                    }).setNegativeButton(android.R.string.cancel, null).show();
        }

        private void showCodec() {
            String[] codecs = getResources().getStringArray(R.array.host_profile_codecs);
            CharSequence[] items = new CharSequence[codecs.length + 1];
            items[0] = getString(R.string.host_profile_same_as_all_value, codecs[activity.global.videoFormat.ordinal()]);
            System.arraycopy(codecs, 0, items, 1, codecs.length);
            PreferenceConfiguration.FormatOption own = activity.overrides.codec;
            new MaterialAlertDialogBuilder(activity).setTitle(R.string.title_video_format)
                    .setSingleChoiceItems(items, own == null ? 0 : own.ordinal() + 1, (dialog, which) -> {
                        activity.overrides.codec = which == 0 ? null :
                                PreferenceConfiguration.FormatOption.values()[which - 1];
                        updateSummaries();
                        dialog.dismiss();
                    }).setNegativeButton(android.R.string.cancel, null).show();
        }

        private void showUpscaling() {
            String[] modes = getResources().getStringArray(R.array.upscaling_names);
            CharSequence[] items = new CharSequence[modes.length + 1];
            items[0] = getString(R.string.host_profile_same_as_all_value, modes[activity.global.upscalingMode.ordinal()]);
            System.arraycopy(modes, 0, items, 1, modes.length);
            UpscalingPolicy.Mode own = activity.overrides.upscalingMode;
            AlertDialog picker = new MaterialAlertDialogBuilder(activity).setTitle(R.string.title_upscaling)
                    .setSingleChoiceItems(items, own == null ? 0 : own.ordinal() + 1, (dialog, which) -> {
                        activity.overrides.upscalingMode = which == 0 ? null : UpscalingPolicy.Mode.values()[which - 1];
                        updateSummaries();
                        dialog.dismiss();
                    }).setNeutralButton(R.string.help, null)
                    .setNegativeButton(android.R.string.cancel, null).create();
            UiHelper.showDialog(activity, picker);
            picker.getButton(AlertDialog.BUTTON_NEUTRAL).setOnClickListener(view ->
                    UiHelper.showDialog(activity, new MaterialAlertDialogBuilder(activity).setTitle(R.string.title_upscaling)
                            .setMessage(R.string.summary_upscaling).setPositiveButton(android.R.string.ok, null).create()));
        }

        private LinearLayout form() {
            LinearLayout form = new LinearLayout(activity);
            form.setOrientation(LinearLayout.VERTICAL);
            int padding = Math.round(24 * getResources().getDisplayMetrics().density);
            form.setPadding(padding, padding / 3, padding, 0);
            return form;
        }

        private ScrollView scrollable(LinearLayout form) {
            ScrollView scroll = new ScrollView(activity);
            scroll.setFillViewport(true);
            scroll.setClipToPadding(false);
            scroll.addView(form);
            return scroll;
        }

        private void showDisplay() {
            AlertDialog picker = HostDisplaySettings.create(activity, activity.hostUuid,
                    activity.getIntent().getStringExtra("host_name"), this::updateSummaries);
            showEditor("display", picker);
            HostDisplaySettings.bindHelp(activity, picker);
        }

        private void showEditor(String key, AlertDialog dialog) {
            activity.editorKey = key;
            activity.editorDialog = dialog;
            UiHelper.showDialog(activity, dialog);
            if (key.equals(activity.restoredEditorKey) && activity.restoredEditor != null) {
                dialog.onRestoreInstanceState(activity.restoredEditor);
                activity.restoredEditor = null;
                activity.restoredEditorKey = null;
            }
        }

        @Override
        public void onResume() {
            super.onResume();
            String key = activity.restoredEditorKey;
            if (key == null) return;
            switch (key) {
                case "display": showDisplay(); break;
                case "resolution": showResolution(); break;
                case "sharpness": showSharpness(); break;
                case "refresh": showNumber(key, R.string.host_profile_refresh); break;
                case "bitrate": showNumber(key, R.string.host_profile_bitrate); break;
                case "scale": showNumber(key, R.string.host_profile_scale); break;
                default: activity.restoredEditorKey = null; break;
            }
        }

        /** The "Same as all PCs" box on top of a value dialog; checked, it disables {@code fields}. */
        private MaterialCheckBox sameAsAll(LinearLayout form, String allPcsValue, boolean checked, View... fields) {
            MaterialCheckBox box = new MaterialCheckBox(activity);
            box.setId(R.id.host_profile_follow_all);
            box.setText(getString(R.string.host_profile_same_as_all_value, allPcsValue));
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

        private EditText field(LinearLayout form, int title, String value, boolean decimal) {
            TextInputLayout container = new TextInputLayout(activity);
            container.setHint(title);
            EditText field = new TextInputEditText(container.getContext());
            field.setId(form.findViewById(R.id.host_profile_field_primary) == null ?
                    R.id.host_profile_field_primary : R.id.host_profile_field_secondary);
            field.setContentDescription(getString(title));
            field.setInputType(InputType.TYPE_CLASS_NUMBER | (decimal ? InputType.TYPE_NUMBER_FLAG_DECIMAL : 0));
            field.setSingleLine(true);
            field.setSelectAllOnFocus(true);
            field.setMinHeight(Math.round(56 * getResources().getDisplayMetrics().density));
            field.setText(value);
            container.addView(field);
            form.addView(container);
            return field;
        }

        // Inline under the field (a TextInputLayout error), so it stays visible and screen readers announce it.
        private void showError(EditText field, String error) {
            View parent = (View) field.getParent();
            while (!(parent instanceof TextInputLayout)) parent = (View) parent.getParent();
            ((TextInputLayout) parent).setError(error);
        }

        private void showSharpness() {
            PreferenceConfiguration config = activity.effective();
            LinearLayout form = form();
            TextView value = new TextView(activity);
            Slider slider = new Slider(activity);
            slider.setId(R.id.host_profile_sharpness);
            MaterialCheckBox allPcs = sameAsAll(form, getString(R.string.upscaling_sharpness_value,
                    activity.global.upscalingSharpness), activity.overrides.upscalingSharpness == null, slider);
            value.setText(getString(R.string.upscaling_sharpness_value, config.upscalingSharpness));
            value.setTextSize(24);
            form.addView(value);
            slider.setContentDescription(getString(R.string.title_upscaling_sharpness));
            slider.setValueFrom(0);
            slider.setValueTo(100);
            slider.setStepSize(1);
            slider.setValue(config.upscalingSharpness);
            slider.setEnabled(!allPcs.isChecked());
            slider.addOnChangeListener((view, strength, fromUser) ->
                    value.setText(getString(R.string.upscaling_sharpness_value, Math.round(strength))));
            form.addView(slider);
            showEditor("sharpness", new MaterialAlertDialogBuilder(activity).setTitle(R.string.title_upscaling_sharpness).setView(scrollable(form))
                    .setPositiveButton(android.R.string.ok, (dialog, which) -> {
                        activity.overrides.upscalingSharpness = allPcs.isChecked() ? null : Math.round(slider.getValue());
                        updateSummaries();
                    }).setNegativeButton(android.R.string.cancel, null).create());
        }

        private void showResolution() {
            PreferenceConfiguration config = activity.effective();
            HostStreamProfile own = activity.overrides;
            LinearLayout form = form();
            MaterialCheckBox nativeChoice = new MaterialCheckBox(activity);
            nativeChoice.setId(R.id.host_profile_follow_screen);
            nativeChoice.setText(R.string.host_profile_native);
            nativeChoice.setChecked(config.resolutionFollowsScreen);
            EditText[] size = new EditText[2];
            MaterialCheckBox allPcs = sameAsAll(form, resolutionText(activity.global), own.nativeResolution == null,
                    nativeChoice);
            form.addView(nativeChoice);
            size[0] = field(form, R.string.host_profile_width, Integer.toString(config.width), false);
            size[1] = field(form, R.string.host_profile_height, Integer.toString(config.height), false);
            Runnable refresh = () -> {
                nativeChoice.setEnabled(!allPcs.isChecked());
                enable(!allPcs.isChecked() && !nativeChoice.isChecked(), size);
            };
            allPcs.setOnCheckedChangeListener((button, checked) -> refresh.run());
            nativeChoice.setOnCheckedChangeListener((button, checked) -> refresh.run());
            refresh.run();
            AlertDialog dialog = new MaterialAlertDialogBuilder(activity).setTitle(R.string.title_resolution_list)
                    .setView(scrollable(form)).setPositiveButton(android.R.string.ok, null)
                    .setNegativeButton(android.R.string.cancel, null)
                    .setNeutralButton(R.string.host_profile_match_screen, null).create();
            dialog.setOnShowListener(ignored -> {
                dialog.getButton(AlertDialog.BUTTON_NEUTRAL).setOnClickListener(v -> {
                    allPcs.setChecked(false);
                    nativeChoice.setChecked(false);
                    DisplayMetrics metrics = new DisplayMetrics();
                    activity.getWindowManager().getDefaultDisplay().getRealMetrics(metrics);
                    size[0].setText(Integer.toString(Math.max(metrics.widthPixels, metrics.heightPixels)));
                    size[1].setText(Integer.toString(Math.min(metrics.widthPixels, metrics.heightPixels)));
                });
                dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(v -> {
                    if (allPcs.isChecked() || nativeChoice.isChecked()) {
                        own.nativeResolution = allPcs.isChecked() ? null : Boolean.TRUE;
                        own.width = own.height = null;
                        updateSummaries();
                        dialog.dismiss();
                        return;
                    }
                    Integer w = readNumber(size[0], 64, 16384);
                    Integer h = readNumber(size[1], 64, 16384);
                    if (w != null && h != null) {
                        own.nativeResolution = false;
                        own.width = w;
                        own.height = h;
                        updateSummaries();
                        dialog.dismiss();
                    }
                });
            });
            showEditor("resolution", dialog);
        }

        private void showNumber(String key, int title) {
            PreferenceConfiguration config = activity.effective();
            PreferenceConfiguration global = activity.global;
            HostStreamProfile own = activity.overrides;
            LinearLayout form = form();
            String value, allPcsValue;
            boolean followsAll;
            if (key.equals("refresh")) {
                value = HostStreamProfile.formatRefreshRate(config.launchRefreshRateX100 == 0 ?
                        config.fps * 100 : config.launchRefreshRateX100);
                allPcsValue = refreshText(global);
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
            MaterialCheckBox allPcs = sameAsAll(form, allPcsValue, followsAll);
            field[0] = field(form, title, value, !key.equals("scale"));
            allPcs.setOnCheckedChangeListener((button, checked) -> field[0].setEnabled(!checked));
            field[0].setEnabled(!followsAll);
            MaterialAlertDialogBuilder builder = new MaterialAlertDialogBuilder(activity).setTitle(title).setView(scrollable(form))
                    .setPositiveButton(android.R.string.ok, null).setNegativeButton(android.R.string.cancel, null);
            if (key.equals("refresh")) builder.setNeutralButton(R.string.host_profile_match_screen, null);
            AlertDialog dialog = builder.create();
            dialog.setOnShowListener(ignored -> {
                if (key.equals("refresh")) {
                    dialog.getButton(AlertDialog.BUTTON_NEUTRAL).setOnClickListener(v -> {
                        allPcs.setChecked(false);
                        field[0].setText(HostStreamProfile.formatRefreshRate(Math.round(100 *
                                com.limelight.binding.video.DisplayFrameRatePolicy.maxRefreshRate(
                                        activity.getWindowManager().getDefaultDisplay()))));
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
                            Integer scale = readNumber(field[0], 50, 200);
                            if (scale == null) return;
                            own.renderScale = scale;
                        }
                        updateSummaries();
                        dialog.dismiss();
                    } catch (IllegalArgumentException e) {
                        showError(field[0], getString(key.equals("refresh") ? R.string.host_profile_invalid_refresh : R.string.host_profile_invalid_bitrate));
                    }
                });
            });
            showEditor(key, dialog);
        }

        private Integer readNumber(EditText field, int min, int max) {
            showError(field, null);
            try {
                return HostStreamProfile.requireRange(Integer.parseInt(field.getText().toString().trim()), min, max);
            } catch (IllegalArgumentException e) {
                showError(field, getString(R.string.host_profile_invalid_number, min, max));
                return null;
            }
        }

        @Override
        public View onCreateView(LayoutInflater inflater, ViewGroup container, Bundle state) {
            View view = super.onCreateView(inflater, container, state);
            ListView list = view.findViewById(android.R.id.list);
            com.limelight.ui.UiNavigation.bindPreferences(list);
            list.setNextFocusDownId(R.id.host_profile_save);
            list.setDivider(null);
            int padding = Math.round(8 * getResources().getDisplayMetrics().density);
            list.setPadding(padding, 0, padding, padding);
            return view;
        }
    }
}

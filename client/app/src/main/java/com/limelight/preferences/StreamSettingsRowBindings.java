package com.limelight.preferences;

import android.Manifest;
import android.app.Activity;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.os.Build;
import android.os.Vibrator;
import android.preference.ListPreference;
import android.preference.Preference;
import android.preference.PreferenceCategory;
import android.preference.PreferenceScreen;
import android.preference.SwitchPreference;
import android.widget.Toast;

import androidx.appcompat.app.AlertDialog;

import com.limelight.BuildConfig;
import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.binding.input.touch.TouchMode;
import com.limelight.binding.video.PyroWaveDecoderRenderer;
import com.limelight.ui.AppTheme;
import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.ui.StreamOrientation;
import com.limelight.ui.VideoScaling;
import com.limelight.update.UpdateFlow;
import com.limelight.update.UpdatePrefs;
import com.limelight.utils.HelpLauncher;
import com.limelight.utils.UiHelper;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

/**
 * Preference row listeners, choice dialogs, hardware feature pruning, and saved-PC test rows for
 * {@link StreamSettings.SettingsFragment}.
 */
final class StreamSettingsRowBindings {
    private StreamSettingsRowBindings() { }

    static String networkTestsSummary(Context context) {
        return context.getString(R.string.summary_network_tests);
    }

    /** The saved PCs by name: the network tests are a PC's own, on its settings screen. Reads the database. */
    private static List<com.limelight.nvstream.http.ComputerDetails> savedPcs(Context context) {
        List<com.limelight.nvstream.http.ComputerDetails> pcs;
        com.limelight.computers.ComputerDatabaseManager database =
                new com.limelight.computers.ComputerDatabaseManager(context);
        try {
            pcs = new ArrayList<>(database.getAllComputers());
        } finally {
            database.close();
        }
        // The name the user gave a PC, as on Home.
        for (com.limelight.nvstream.http.ComputerDetails pc : pcs) pc.alias = com.limelight.computers.HostNames.get(context, pc.uuid);
        pcs.sort((a, b) -> String.CASE_INSENSITIVE_ORDER.compare(String.valueOf(a.displayName()),
                String.valueOf(b.displayName())));
        return pcs;
    }

    /**
     * The connection test needs a saved PC, and the PyroWave frame pace a saved PC that can measure it or has
     * one saved: without them the rows are hidden (they leave the More count and search).
     */
    static void refreshNetworkTests(StreamSettingsFragment fragment) {
        withSavedPcs(fragment, (pcs, pace) -> {
            StreamSettings activity = (StreamSettings) fragment.getActivity();
            activity.anySavedPc = !pcs.isEmpty();
            activity.anyPacePc = !pace.isEmpty();
            activity.anyJumboPc = anyJumboPc(pcs);
            if (showKnownPcRows(fragment)) {
                fragment.placeRows();
                // Results already on screen were found without the rows' new state.
                if (!activity.searchQuery.isEmpty()) fragment.filterPreferences();
            }
        });
    }

    /**
     * Shows or hides the rows as the last read of the saved PCs found (hidden before the first answers); true when
     * that changed them.
     */
    static boolean showKnownPcRows(StreamSettingsFragment fragment) {
        StreamSettings activity = (StreamSettings) fragment.getActivity();
        boolean testsHidden = fragment.hidden.contains(StreamSettings.NETWORK_TESTS_KEY);
        boolean paceHidden = fragment.hidden.contains(StreamSettings.PACE_CALIBRATION_KEY);
        boolean jumboHidden = fragment.hidden.contains(PreferenceConfiguration.JUMBO_FRAMES_PREF_STRING);
        fragment.setShown(StreamSettings.NETWORK_TESTS_KEY, activity.anySavedPc);
        fragment.setShown(StreamSettings.PACE_CALIBRATION_KEY, activity.anyPacePc);
        // Jumbo frames only where a saved PC offers more than stock packets.
        fragment.setShown(PreferenceConfiguration.JUMBO_FRAMES_PREF_STRING, activity.anyJumboPc);
        return testsHidden != fragment.hidden.contains(StreamSettings.NETWORK_TESTS_KEY)
                || paceHidden != fragment.hidden.contains(StreamSettings.PACE_CALIBRATION_KEY)
                || jumboHidden != fragment.hidden.contains(PreferenceConfiguration.JUMBO_FRAMES_PREF_STRING);
    }

    /** Whether any saved PC reported a jumbo video packet offer. */
    private static boolean anyJumboPc(List<com.limelight.nvstream.http.ComputerDetails> pcs) {
        for (com.limelight.nvstream.http.ComputerDetails pc : pcs) {
            if (pc.caps != null && pc.caps.maxVideoPacket() > 0) return true;
        }
        return false;
    }

    /**
     * Reads the saved PCs off the UI thread, and hands them and the ones whose frame pace can be measured to
     * {@code then} on it; nothing happens when this screen is gone by then.
     */
    static void withSavedPcs(StreamSettingsFragment fragment,
            java.util.function.BiConsumer<List<com.limelight.nvstream.http.ComputerDetails>,
                    List<com.limelight.nvstream.http.ComputerDetails>> then) {
        StreamSettings activity = (StreamSettings) fragment.getActivity();
        if (activity == null) return;
        Context context = activity.getApplicationContext();
        activity.savedPcReader.execute(() -> {
            List<com.limelight.nvstream.http.ComputerDetails> pcs;
            List<com.limelight.nvstream.http.ComputerDetails> pace;
            try {
                pcs = savedPcs(context);
                pace = pacePcs(context, pcs);
            } catch (RuntimeException e) {
                LimeLog.warning("Settings could not read the saved PCs: " + e);
                return;
            }
            activity.runOnUiThread(() -> {
                if (fragment.isAdded() && fragment.getActivity() == activity && !activity.isFinishing()) then.accept(pcs, pace);
            });
        });
    }

    /** The saved PCs whose PyroWave frame pace can be measured, or that have one saved. */
    private static List<com.limelight.nvstream.http.ComputerDetails> pacePcs(Context context,
            List<com.limelight.nvstream.http.ComputerDetails> pcs) {
        List<com.limelight.nvstream.http.ComputerDetails> pace = new ArrayList<>();
        for (com.limelight.nvstream.http.ComputerDetails pc : pcs) {
            if (pc.supportsPyroWavePaceCalibration() ||
                    com.limelight.utils.PyroWavePaceStore.get(context, pc.uuid) != null) pace.add(pc);
        }
        return pace;
    }

    /** One PC opens its tests, where its frame pace is measured; several ask which PC first. */
    static void openPcTests(StreamSettingsFragment fragment, List<com.limelight.nvstream.http.ComputerDetails> pcs, int title) {
        Activity activity = fragment.getActivity();
        if (pcs.isEmpty()) {
            refreshNetworkTests(fragment);
            return;
        }
        if (pcs.size() == 1) {
            HostStreamSettings.showTests(activity, pcs.get(0).uuid, pcs.get(0).name);
            return;
        }
        String[] names = new String[pcs.size()];
        for (int i = 0; i < names.length; i++) names[i] = pcs.get(i).displayName();
        UiHelper.showDialog(activity, new NavigationDialogBuilder(activity)
                .setTitle(title)
                .setItems(names, (dialog, which) ->
                        HostStreamSettings.showTests(activity, pcs.get(which).uuid, pcs.get(which).name))
                .setNegativeButton(android.R.string.cancel, null)
                .create());
    }

    /** One saved PC opens its tests; several ask which PC first. */
    private static void openNetworkTests(StreamSettingsFragment fragment) {
        withSavedPcs(fragment, (pcs, pace) -> openPcTests(fragment, pcs, R.string.network_tests_title));
    }

    static void showInfo(Activity activity, int title, int message) {
        UiHelper.showDialog(activity, new NavigationDialogBuilder(activity).setTitle(title)
                .setMessage(message).setPositiveButton(android.R.string.ok, null).create());
    }

    /**
     * A key row of Artwork and game info: opens a text box holding the key. The key is kept in the game_info_keys
     * file, never in the default preferences, and never shown in the row, whose summary is its help.
     */
    private static void bindGameInfoKey(StreamSettingsFragment fragment, String row, String storedAs, int title) {
        fragment.findPreference(row).setOnPreferenceClickListener(preference -> {
            Activity activity = fragment.getActivity();
            SharedPreferences keys = com.limelight.console.GameInfo.keyPrefs(activity);
            android.widget.EditText input = new android.widget.EditText(activity);
            input.setSingleLine(true);
            input.setInputType(android.text.InputType.TYPE_CLASS_TEXT | android.text.InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
                    | android.text.InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD);
            input.setText(keys.getString(storedAs, ""));
            input.setSelection(input.getText().length());
            AlertDialog dialog = new NavigationDialogBuilder(activity).setTitle(title).setView(input)
                    .setPositiveButton(android.R.string.ok, (d, which) -> {
                        String value = input.getText().toString().trim();
                        if (value.isEmpty()) keys.edit().remove(storedAs).apply();
                        else keys.edit().putString(storedAs, value).apply();
                        // The client ID shows the secret's row; the keys live outside the settings' listener.
                        if (fragment.isAdded()) fragment.refreshState();
                    })
                    .setNegativeButton(android.R.string.cancel, null).create();
            UiHelper.showDialog(activity, dialog);
            return true;
        });
    }

    private static void showAbout(Activity activity) {
        AlertDialog about = new NavigationDialogBuilder(activity).setTitle(R.string.title_about)
                .setMessage(activity.getString(R.string.summary_about, BuildConfig.VERSION_NAME) + "\n\n"
                        + activity.getString(R.string.game_info_credits))
                .setPositiveButton(android.R.string.ok, null)
                .setNeutralButton(R.string.settings_source, (dialog, which) ->
                        HelpLauncher.launchUrl(activity, "https://github.com/RamazanKara/nightjar"))
                .create();
        UiHelper.showDialog(activity, about);
    }

    /** A one-of-several picker for a row that is not a list preference; Help explains the choices when given. */
    private static void showChoices(Activity activity, int title, int names, int selected, int help,
            java.util.function.IntConsumer onPick) {
        showChoices(activity, title, activity.getResources().getTextArray(names), selected, help, onPick);
    }

    private static void showChoices(Activity activity, int title, CharSequence[] names, int selected, int help,
            java.util.function.IntConsumer onPick) {
        com.google.android.material.dialog.MaterialAlertDialogBuilder builder = new NavigationDialogBuilder(activity).setTitle(title)
                .setSingleChoiceItems(names, selected, (dialog, which) -> {
                    onPick.accept(which);
                    dialog.dismiss();
                }).setNegativeButton(android.R.string.cancel, null);
        if (help != 0) builder.setNeutralButton(R.string.help, null);
        AlertDialog picker = builder.create();
        UiHelper.showDialog(activity, picker);
        if (help != 0) {
            picker.getButton(AlertDialog.BUTTON_NEUTRAL).setOnClickListener(view -> showInfo(activity, title, help));
        }
    }

    private static void confirmReset(StreamSettingsFragment fragment) {
        Activity activity = fragment.getActivity();
        AlertDialog confirm = new NavigationDialogBuilder(activity).setTitle(R.string.dialog_reset_settings_title)
                .setMessage(R.string.dialog_reset_settings_text)
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(R.string.dialog_reset_settings_confirm, (dialog, which) -> {
                    StreamSettings.resetAllSettings(activity);
                    ((StreamSettings) activity).reloadSettings();
                    Toast.makeText(activity, R.string.toast_reset_settings, Toast.LENGTH_SHORT).show();
                }).create();
        UiHelper.showDialog(activity, confirm);
        UiHelper.markDestructive(confirm);
    }

    /** Binds row listeners/dialogs and prunes hardware-inapplicable rows; returns the HDR toggle before pruning. */
    static SwitchPreference bindRowsAndPruneHardware(StreamSettingsFragment fragment, PreferenceScreen screen,
            SharedPreferences settings) {
        StreamSettings activity = (StreamSettings) fragment.getActivity();
        if (!settings.contains(PreferenceConfiguration.BITRATE_USER_SET_PREF_STRING)) {
            // Decide for older installs before a removed resolution or frame rate is replaced below.
            settings.edit().putBoolean(PreferenceConfiguration.BITRATE_USER_SET_PREF_STRING,
                    PreferenceConfiguration.isBitrateUserSet(settings)).apply();
        }
        fragment.findPreference(PreferenceConfiguration.BITRATE_PREF_STRING).setOnPreferenceChangeListener((preference, value) -> {
            settings.edit().putBoolean(PreferenceConfiguration.BITRATE_USER_SET_PREF_STRING, true).apply();
            return true;
        });
        fragment.findPreference(StreamSettings.AUTOMATIC_BITRATE_KEY).setOnPreferenceChangeListener((preference, value) -> {
            StreamSettingsDisplayOptions.setAutomaticBitrate(settings, (Boolean) value);
            return true;
        });
        fragment.findPreference("checkbox_phone_performance_hints").setEnabled(Build.VERSION.SDK_INT >= Build.VERSION_CODES.S);
        if (!PyroWaveDecoderRenderer.hasAdrenoGpu()) {
            fragment.removeSetting(PreferenceConfiguration.GPU_MAX_CLOCKS_PREF_STRING);
        }
        // The same check as the driver screen: custom drivers load on arm64 Adreno devices only.
        if (!activity.supportsGpuDrivers()) {
            fragment.removeSetting("gpu_driver");
        }
        fragment.findPreference(StreamSettings.LOW_LATENCY_DECODING_KEY).setOnPreferenceChangeListener((preference, value) -> {
            PreferenceConfiguration.setLowLatencyDecoding(fragment.getPreferenceManager().getSharedPreferences(), (Boolean) value);
            return true;
        });
        Preference lowerBitrate = fragment.findPreference(AutoBitrateChoice.SETTINGS_KEY);
        if (lowerBitrate != null) {
            lowerBitrate.setOnPreferenceChangeListener((preference, value) -> {
                // Every PC follows the choice again, including PCs that chose otherwise in the stream menu.
                AutoBitrateChoice.clearPcChoices(fragment.getActivity());
                return true;
            });
        }
        for (SwitchPairChoice pair : SwitchPairChoice.values()) {
            Preference row = fragment.findPreference(pair.key);
            if (row == null) continue;
            row.setOnPreferenceClickListener(preference -> {
                SharedPreferences prefs = fragment.getPreferenceManager().getSharedPreferences();
                CharSequence[] all = fragment.getResources().getTextArray(pair.names);
                int[] order = pair.displayOrder(pair.offered(fragment.getActivity(), prefs));
                CharSequence[] names = new CharSequence[order.length];
                int current = 0;
                for (int i = 0; i < order.length; i++) {
                    names[i] = all[order[i]];
                    if (order[i] == pair.read(prefs)) current = i;
                }
                showChoices(fragment.getActivity(), pair.title, names, current, pair.help, which -> {
                    SharedPreferences.Editor editor = prefs.edit();
                    pair.save(editor, order[which]);
                    editor.apply();
                });
                return true;
            });
        }
        ListPreference codec = (ListPreference) fragment.findPreference("video_format");
        SwitchPreference hdr = (SwitchPreference) fragment.findPreference("checkbox_enable_hdr");
        SwitchPreference sdr10 = (SwitchPreference) fragment.findPreference("checkbox_enable_sdr_10bit");
        Preference texture = fragment.findPreference("checkbox_texture_view");
        // A device that can never run PyroWave says why in the list; its GPU is asked once chosen.
        int noPyroWave = PyroWaveDecoderRenderer.neverAvailableReason();
        if (noPyroWave != 0) StreamSettingsDisplayOptions.labelEntry(codec, "forcepyrowave", fragment.getString(noPyroWave));
        texture.setEnabled(!hdr.isChecked() && !sdr10.isChecked() && !"forcepyrowave".equals(codec.getValue()));
        codec.setOnPreferenceChangeListener((preference, value) -> {
            texture.setEnabled(!hdr.isChecked() && !sdr10.isChecked() && !"forcepyrowave".equals(value));
            StreamSettingsSearchAndSummaries.updateCodecSummary(fragment, codec, (String) value, hdr.isChecked());
            StreamSettingsDisplayOptions.resetBitrateToDefault(settings, null, null, (String) value);
            return true;
        });
        hdr.setOnPreferenceChangeListener((preference, value) -> {
            texture.setEnabled(!(Boolean) value && !sdr10.isChecked() && !"forcepyrowave".equals(codec.getValue()));
            StreamSettingsSearchAndSummaries.updateCodecSummary(fragment, codec, codec.getValue(), (Boolean) value);
            return true;
        });
        sdr10.setOnPreferenceChangeListener((preference, value) -> {
            texture.setEnabled(!hdr.isChecked() && !(Boolean) value && !"forcepyrowave".equals(codec.getValue()));
            return true;
        });
        // The simple layer: a read-only line for the screen it streams at, and the one-tap way back.
        Preference simpleScreen = fragment.findPreference("simple_screen");
        if (simpleScreen != null) {
            simpleScreen.setOnPreferenceClickListener(preference -> {
                ((StreamSettings) fragment.getActivity()).showSection("display");
                return true;
            });
        }
        Preference recommended = fragment.findPreference("simple_back_to_recommended");
        if (recommended != null) {
            recommended.setOnPreferenceClickListener(preference -> {
                if (!SimpleProfile.applyRecommended(fragment.getActivity())) {
                    Toast.makeText(fragment.getActivity(), R.string.simple_profile_failed, Toast.LENGTH_LONG).show();
                }
                fragment.updateSimpleScreenSummary();
                return true;
            });
        }
        Preference simpleHdr = fragment.findPreference("simple_hdr");
        if (simpleHdr != null && !SimpleProfile.hdrOffered(activity)) fragment.removeSetting("simple_hdr");
        fragment.findPreference(StreamSettings.SHARP_COLOR_KEY).setOnPreferenceClickListener(preference -> {
            SharedPreferences prefs = fragment.getPreferenceManager().getSharedPreferences();
            showChoices(fragment.getActivity(), R.string.title_sharp_color, R.array.sharp_color_names,
                    PreferenceConfiguration.sharpColor(prefs) ? 1 : 0, R.string.summary_sharp_color, which -> {
                        SharedPreferences.Editor editor = prefs.edit();
                        PreferenceConfiguration.saveSharpColor(editor, which == 1);
                        editor.apply();
                        // Switching it is how to ask again a PC that refused it before, and to hear again
                        // when a stream starts without it.
                        com.limelight.computers.HostPackedModes.clearRefusals(fragment.getActivity());
                        com.limelight.computers.HostNotices.resetEverywhere(fragment.getActivity(),
                                com.limelight.computers.HostNotices.Notice.PACKED_UNAVAILABLE);
                    });
            return true;
        });
        // One switch tells the PC about this device's network: it writes the Wi-Fi and the link speed switches alike.
        Preference tellHost = fragment.findPreference(PreferenceConfiguration.TELL_HOST_WIRELESS_PREF_STRING);
        if (tellHost != null) {
            tellHost.setOnPreferenceChangeListener((preference, value) -> {
                fragment.getPreferenceManager().getSharedPreferences().edit()
                        .putBoolean(PreferenceConfiguration.TELL_HOST_LINK_SPEED_PREF_STRING, (Boolean) value).apply();
                return true;
            });
        }
        fragment.findPreference(StreamSettings.CONTROLLERS_KEY).setOnPreferenceClickListener(preference -> {
            SharedPreferences prefs = fragment.getPreferenceManager().getSharedPreferences();
            showChoices(fragment.getActivity(), R.string.title_controllers, R.array.controllers_names,
                    ControllersChoice.read(prefs), R.string.summary_controllers, which -> {
                        SharedPreferences.Editor editor = prefs.edit();
                        ControllersChoice.save(editor, which);
                        editor.apply();
                    });
            return true;
        });
        Preference mouseMode = fragment.findPreference(StreamSettings.MOUSE_MODE_KEY);
        if (mouseMode != null) {
            mouseMode.setOnPreferenceClickListener(preference -> {
                SharedPreferences prefs = fragment.getPreferenceManager().getSharedPreferences();
                showChoices(fragment.getActivity(), R.string.title_checkbox_absolute_mouse_mode, R.array.mouse_mode_names,
                        prefs.getBoolean(StreamSettings.MOUSE_MODE_KEY, false) ? 1 : 0, R.string.summary_checkbox_absolute_mouse_mode,
                        which -> prefs.edit().putBoolean(StreamSettings.MOUSE_MODE_KEY, which == 1).apply());
                return true;
            });
        }
        fragment.findPreference(com.limelight.console.OpenPlay.SETTING).setOnPreferenceClickListener(preference -> {
            SharedPreferences prefs = fragment.getPreferenceManager().getSharedPreferences();
            // "A chosen game" is there once a game has been chosen, under its title.
            List<String> screens = new ArrayList<>(Arrays.asList(StartScreenChoice.SCREENS));
            if (com.limelight.console.OpenPlay.chosenTitle(fragment.getActivity()) == null) screens.remove(StartScreenChoice.CHOSEN);
            CharSequence[] names = new CharSequence[screens.size()];
            for (int i = 0; i < names.length; i++) {
                names[i] = StreamSettingsSearchAndSummaries.startScreenName(fragment.getActivity(), screens.get(i));
            }
            showChoices(fragment.getActivity(), R.string.title_open_on_start, names, screens.indexOf(StartScreenChoice.read(prefs)),
                    R.string.summary_open_on_start, which -> {
                        SharedPreferences.Editor editor = prefs.edit();
                        StartScreenChoice.save(prefs, editor, screens.get(which));
                        editor.apply();
                    });
            return true;
        });
        fragment.findPreference(StreamSettings.PACE_CALIBRATION_KEY).setOnPreferenceClickListener(preference -> {
            withSavedPcs(fragment, (pcs, pace) -> openPcTests(fragment, pace, R.string.pace_calibration_title));
            return true;
        });
        fragment.findPreference("about_app").setSummary(fragment.getString(R.string.settings_version, BuildConfig.VERSION_NAME));
        fragment.findPreference("about_app").setOnPreferenceClickListener(preference -> {
            showAbout(fragment.getActivity());
            return true;
        });
        bindGameInfoKey(fragment, "game_info_steamgriddb", com.limelight.console.GameInfo.KEY_STEAMGRIDDB, R.string.game_info_sgdb_title);
        bindGameInfoKey(fragment, "game_info_igdb_client_id", com.limelight.console.GameInfo.KEY_IGDB_ID, R.string.game_info_igdb_id_title);
        bindGameInfoKey(fragment, "game_info_igdb_client_secret", com.limelight.console.GameInfo.KEY_IGDB_SECRET, R.string.game_info_igdb_secret_title);
        fragment.findPreference("pin_lock").setOnPreferenceClickListener(preference -> {
            com.limelight.pin.PinLockSettingsActivity.open(fragment.getActivity());
            return true;
        });
        fragment.findPreference("help").setOnPreferenceClickListener(preference -> {
            HelpLauncher.showHelp(fragment.getActivity());
            return true;
        });
        // Google Play, or a build made for another store, updates the app itself.
        if (UpdateFlow.isAvailable(activity)) {
            fragment.findPreference("check_updates_now").setOnPreferenceClickListener(preference -> {
                UpdateFlow.checkNow(fragment.getActivity());
                return true;
            });
        } else {
            fragment.removeSetting(UpdatePrefs.AUTO_CHECK_PREF_STRING);
            fragment.removeSetting("check_updates_now");
        }
        fragment.findPreference(StreamSettings.HOST_DISPLAY_KEY).setOnPreferenceClickListener(preference -> {
            SharedPreferences prefs = fragment.getPreferenceManager().getSharedPreferences();
            showChoices(fragment.getActivity(), R.string.host_display_menu, DisplayArrangementChoice.names(fragment.getActivity(), true),
                    DisplayArrangementChoice.readMain(prefs), R.string.summary_display_arrangement, which -> {
                        SharedPreferences.Editor editor = prefs.edit();
                        DisplayArrangementChoice.saveMain(editor, which);
                        editor.apply();
                    });
            return true;
        });
        fragment.findPreference("latency_benchmark").setOnPreferenceClickListener(preference -> {
            fragment.startActivity(new Intent(fragment.getActivity(), com.limelight.LatencyBenchmarkActivity.class));
            return true;
        });
        // Set before the search descriptions are read: the tests' names find the row.
        Preference networkTests = fragment.findPreference(StreamSettings.NETWORK_TESTS_KEY);
        networkTests.setSummary(networkTestsSummary(activity));
        networkTests.setOnPreferenceClickListener(preference -> {
            openNetworkTests(fragment);
            return true;
        });
        Preference gpuDriver = fragment.findPreference("gpu_driver");
        if (gpuDriver != null) {
            gpuDriver.setOnPreferenceClickListener(preference -> {
                fragment.startActivity(new Intent(fragment.getActivity(), GpuDriverSettings.class));
                return true;
            });
        }
        fragment.findPreference("report_problem").setOnPreferenceClickListener(preference -> {
            com.limelight.utils.ProblemReport.show(fragment.getActivity());
            return true;
        });
        fragment.findPreference("reset_all").setOnPreferenceClickListener(preference -> {
            confirmReset(fragment);
            return true;
        });
        bindPickersAndNavigationRows(fragment);
        pruneUnsupportedHardwareRows(fragment, screen, activity);
        return hdr;
    }

    private static void bindPickersAndNavigationRows(StreamSettingsFragment fragment) {
        fragment.findPreference(TouchMode.PREF).setOnPreferenceClickListener(preference -> {
            SharedPreferences prefs = fragment.getPreferenceManager().getSharedPreferences();
            int selected = StreamSettingsSearchAndSummaries.storedTouchMode(prefs);
            AlertDialog picker = new NavigationDialogBuilder(fragment.getActivity()).setTitle(R.string.stream_touch_mode)
                    .setSingleChoiceItems(R.array.stream_touch_modes, selected, (dialog, which) -> {
                        SharedPreferences.Editor editor = prefs.edit();
                        TouchMode.save(editor, which);
                        editor.apply();
                        dialog.dismiss();
                    }).setNeutralButton(R.string.help, null)
                    .setNegativeButton(android.R.string.cancel, null).create();
            UiHelper.showDialog(fragment.getActivity(), picker);
            // Like the other pickers, Help explains the choices without closing the list.
            picker.getButton(AlertDialog.BUTTON_NEUTRAL).setOnClickListener(view ->
                    showInfo(fragment.getActivity(), R.string.stream_touch_mode, R.string.settings_touch_mode_help));
            return true;
        });
        fragment.findPreference(VideoScaling.PREF).setOnPreferenceClickListener(preference -> {
            SharedPreferences prefs = fragment.getPreferenceManager().getSharedPreferences();
            AlertDialog picker = new NavigationDialogBuilder(fragment.getActivity()).setTitle(R.string.video_scaling_title)
                    .setSingleChoiceItems(R.array.video_scaling_modes, StreamSettingsSearchAndSummaries.storedVideoScaling(prefs), (dialog, which) -> {
                        SharedPreferences.Editor editor = prefs.edit();
                        VideoScaling.save(editor, which);
                        editor.apply();
                        dialog.dismiss();
                    }).setNeutralButton(R.string.help, null)
                    .setNegativeButton(android.R.string.cancel, null).create();
            UiHelper.showDialog(fragment.getActivity(), picker);
            picker.getButton(AlertDialog.BUTTON_NEUTRAL).setOnClickListener(view ->
                    showInfo(fragment.getActivity(), R.string.video_scaling_title, R.string.video_scaling_help));
            return true;
        });
        fragment.findPreference(OverlayChoice.ENABLED_KEY).setOnPreferenceClickListener(preference -> {
            SharedPreferences prefs = fragment.getPreferenceManager().getSharedPreferences();
            showChoices(fragment.getActivity(), R.string.title_performance_overlay, R.array.overlay_choice_names, OverlayChoice.read(prefs),
                    R.string.overlay_mode_help, which -> {
                        SharedPreferences.Editor editor = prefs.edit();
                        OverlayChoice.save(editor, which);
                        editor.apply();
                    });
            return true;
        });
        fragment.findPreference(AppTheme.THEME).setOnPreferenceClickListener(preference -> {
            SharedPreferences prefs = fragment.getPreferenceManager().getSharedPreferences();
            showChoices(fragment.getActivity(), R.string.title_theme, R.array.theme_choice_names, ThemeChoice.read(prefs), 0, which -> {
                SharedPreferences.Editor editor = prefs.edit();
                ThemeChoice.save(editor, which);
                editor.apply();
            });
            return true;
        });
        fragment.findPreference(StreamSettings.LOW_LATENCY_CUSTOMIZE_KEY).setOnPreferenceClickListener(preference -> {
            StreamSettings activity = (StreamSettings) fragment.getActivity();
            activity.customizeOpen = !activity.customizeOpen;
            fragment.refreshState();
            return true;
        });
        for (String moreSection : StreamSettings.SECTIONS.keySet()) {
            Preference more = fragment.findPreference(StreamSettings.MORE_PREFIX + moreSection);
            if (more == null) continue;
            more.setOnPreferenceClickListener(preference -> {
                fragment.toggleMore(moreSection);
                return true;
            });
        }
        fragment.findPreference("controller_button_mapping").setOnPreferenceClickListener(preference -> {
            fragment.startActivity(new Intent(fragment.getActivity(), ControllerMappingActivity.class));
            return true;
        });
        fragment.findPreference("controller_test").setOnPreferenceClickListener(preference -> {
            fragment.startActivity(new Intent(fragment.getActivity(), ControllerTestActivity.class));
            return true;
        });
        fragment.findPreference("gyro_mapping_profiles").setOnPreferenceClickListener(preference -> {
            GyroProfileEditor.open(fragment.getActivity());
            return true;
        });
        fragment.findPreference("keyboard_shortcuts_editor").setOnPreferenceClickListener(preference -> {
            fragment.startActivity(new Intent(fragment.getActivity(), KeyboardShortcutsActivity.class));
            return true;
        });
        fragment.findPreference("stream_hotkeys").setOnPreferenceClickListener(preference -> {
            fragment.startActivity(new Intent(fragment.getActivity(), StreamHotkeysActivity.class));
            return true;
        });
        fragment.findPreference("export_latency_csv").setOnPreferenceClickListener(new Preference.OnPreferenceClickListener() {
            @Override
            public boolean onPreferenceClick(Preference preference) {
                ((StreamSettings) fragment.getActivity()).ui.exportLatencyCsv();
                return true;
            }
        });
    }

    private static void pruneUnsupportedHardwareRows(StreamSettingsFragment fragment, PreferenceScreen screen,
            StreamSettings activity) {
        // A TV does not turn.
        if (activity.getPackageManager().hasSystemFeature(PackageManager.FEATURE_LEANBACK)) {
            fragment.removeSetting(StreamOrientation.PREF);
        }

        // Mobile data rows only where there is mobile data, unless a value set earlier still acts (roaming's
        // choice of it likewise, SwitchPairChoice.offered).
        if (!SettingRules.hasMobileData(activity)) {
            SharedPreferences stored = fragment.getPreferenceManager().getSharedPreferences();
            if (!SettingRules.mobileBitrateShown(activity, stored)) {
                fragment.removeSetting(PreferenceConfiguration.MOBILE_BITRATE_PREF_STRING);
            }
        }

        // hide on-screen controls category on non touch screen devices
        if (!activity.getPackageManager().hasSystemFeature(PackageManager.FEATURE_TOUCHSCREEN)) {
            PreferenceCategory category =
                    (PreferenceCategory) fragment.findPreference("category_onscreen_controls");
            screen.removePreference(category);
            fragment.removeSetting("checkbox_show_onscreen_controls");
            // The floating menu button is for touch; a TV opens the menu from the remote or controller.
            PreferenceCategory menu = (PreferenceCategory) fragment.findPreference("category_stream_menu");
            Preference floating = fragment.findPreference(PreferenceConfiguration.FLOATING_MENU_BUTTON_PREF_STRING);
            if (menu != null && floating != null) menu.removePreference(floating);
        }

        // Hide remote desktop mouse mode on pre-Oreo (which doesn't have pointer capture)
        // and NVIDIA SHIELD devices (which support raw mouse input in pointer capture mode)
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O ||
                activity.getPackageManager().hasSystemFeature("com.nvidia.feature.shield")) {
            fragment.removeSetting("checkbox_absolute_mouse_mode");
        }

        // Hide gamepad motion sensor option when running on OSes before Android 12.
        // Support for motion, LED, battery, and other extensions were introduced in S.
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) {
            fragment.removeSetting("checkbox_gamepad_motion_sensors");
        }

        // Hide gamepad motion sensor fallback option if the device has no gyro or accelerometer
        if (!activity.getPackageManager().hasSystemFeature(PackageManager.FEATURE_SENSOR_ACCELEROMETER) &&
                !activity.getPackageManager().hasSystemFeature(PackageManager.FEATURE_SENSOR_GYROSCOPE)) {
            fragment.removeSetting("checkbox_gamepad_motion_fallback");
        }

        // The microphone switch turns on only once microphone access is allowed.
        SwitchPreference mic = (SwitchPreference) fragment.findPreference(PreferenceConfiguration.ENABLE_MIC_PREF_STRING);
        mic.setOnPreferenceChangeListener((preference, value) -> {
            if (!Boolean.TRUE.equals(value) || fragment.hasMicPermission()) {
                return true;
            }
            fragment.requestPermissions(new String[] {Manifest.permission.RECORD_AUDIO}, StreamSettingsFragment.MIC_PERMISSION_REQUEST);
            return false;
        });

        SwitchPreference dualSense = (SwitchPreference) fragment.findPreference("checkbox_usb_dualsense");
        dualSense.setOnPreferenceChangeListener((preference, value) -> {
            if (!Boolean.TRUE.equals(value)) {
                return true;
            }
            new NavigationDialogBuilder(fragment.getActivity())
                    .setTitle(R.string.title_usb_dualsense)
                    .setMessage(R.string.usb_dualsense_permission_info)
                    .setPositiveButton(R.string.usb_dualsense_enable, (dialog, which) -> dualSense.setChecked(true))
                    .setNegativeButton(android.R.string.cancel, null)
                    .show();
            return false;
        });

        // Hide USB driver options on devices without USB host support
        if (!activity.supportsUsbHost()) {
            screen.removePreference(fragment.findPreference("category_usb_controllers"));
        }

        // Remove PiP mode on devices pre-Oreo, where the feature is not available (some low RAM devices),
        // and on Fire OS where it violates the Amazon App Store guidelines for some reason.
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O ||
                !activity.getPackageManager().hasSystemFeature("android.software.picture_in_picture") ||
                activity.getPackageManager().hasSystemFeature("com.amazon.software.fireos")) {
            fragment.removeSetting("checkbox_enable_pip");
        }

        // Remove the vibration options if the device can't vibrate
        if (!((Vibrator) activity.getSystemService(Context.VIBRATOR_SERVICE)).hasVibrator()) {
            fragment.removeSetting("checkbox_vibrate_fallback");
            fragment.removeSetting("seekbar_vibrate_fallback_strength");
            // The entire OSC category may have already been removed by the touchscreen check above
            fragment.removeSetting("checkbox_vibrate_osc");
        } else if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O ||
                !((Vibrator) activity.getSystemService(Context.VIBRATOR_SERVICE)).hasAmplitudeControl()) {
            // Remove the vibration strength selector of the device doesn't have amplitude control
            fragment.removeSetting("seekbar_vibrate_fallback_strength");
        }
    }
}

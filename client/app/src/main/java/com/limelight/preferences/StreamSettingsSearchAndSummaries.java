package com.limelight.preferences;

import android.app.Activity;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.os.Build;
import android.preference.ListPreference;
import android.preference.Preference;
import android.preference.PreferenceCategory;
import android.preference.PreferenceGroup;
import android.preference.PreferenceScreen;
import android.preference.SwitchPreference;
import android.text.TextUtils;
import android.widget.TextView;

import com.limelight.R;
import com.limelight.binding.input.touch.TouchMode;
import com.limelight.binding.video.DecoderBenchmark;
import com.limelight.binding.video.GpuDriverStore;
import com.limelight.binding.video.PyroWaveDecoderRenderer;
import com.limelight.ui.AppTheme;
import com.limelight.ui.VideoScaling;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.Map;

/**
 * Search result filtering, live value summaries, codec help messages, and when-reason annotations
 * for {@link StreamSettings.SettingsFragment}.
 */
final class StreamSettingsSearchAndSummaries {
    private StreamSettingsSearchAndSummaries() { }

    // Every category is built first so the device checks below can find their preferences
    static void filterPreferences(StreamSettingsFragment fragment) {
        StreamSettings activity = (StreamSettings) fragment.getActivity();
        PreferenceScreen screen = fragment.getPreferenceManager().createPreferenceScreen(activity);
        String query = activity.searchQuery;
        String section = StreamSettings.normalizeSection(
                fragment.getArguments() == null ? null : fragment.getArguments().getString(StreamSettingsFragment.ARG_SECTION));
        String[] categories = section == null ? null : StreamSettings.SECTIONS.get(section);
        int resultCount = 0;
        if (!query.isEmpty() && SettingsSearch.matches(query, fragment.getString(R.string.stream_presets),
                fragment.getString(R.string.stream_presets_help),
                String.join(" ", fragment.getResources().getStringArray(R.array.stream_preset_names)))) {
            Preference presets = new Preference(activity);
            presets.setTitle(R.string.stream_presets);
            presets.setOnPreferenceClickListener(pref -> {
                ((StreamSettings) fragment.getActivity()).openSearchResult(null, "stream_presets");
                return true;
            });
            screen.addPreference(presets);
            resultCount++;
        }
        // Profiles live on the overview too, outside every section's rows: their result opens them.
        if (!query.isEmpty() && SettingsSearch.matches(query, fragment.getString(R.string.profiles_title),
                profilesSummary(activity), null)) {
            Preference profiles = new Preference(activity);
            profiles.setKey("search_result_" + StreamSettings.PROFILES_KEY);
            profiles.setTitle(R.string.profiles_title);
            profiles.setSummary(profilesSummary(activity));
            profiles.setOnPreferenceClickListener(pref -> {
                fragment.startActivity(new Intent(fragment.getActivity(), StreamProfilesActivity.class));
                return true;
            });
            screen.addPreference(profiles);
            resultCount++;
        }
        for (Map.Entry<PreferenceCategory, List<Preference>> entry : fragment.searchCategories.entrySet()) {
            PreferenceCategory category = entry.getKey();
            if (query.isEmpty()) {
                // A group whose rows are all folded waits for its More row (showGroups).
                if ((categories == null || Arrays.asList(categories).contains(category.getKey())) &&
                        category.getPreferenceCount() > 0) {
                    screen.addPreference(category);
                }
                continue;
            }
            final String resultSection = StreamSettings.sectionOf(category.getKey());
            PreferenceCategory results = new PreferenceCategory(activity);
            CharSequence sectionTitle = resultSection == null ? null : fragment.getString(StreamSettings.sectionTitle(resultSection));
            results.setTitle(sectionTitle == null ? category.getTitle() :
                    TextUtils.isEmpty(category.getTitle()) ? sectionTitle :
                    fragment.getString(R.string.settings_search_breadcrumb, sectionTitle, category.getTitle()));
            screen.addPreference(results);
            for (Preference pref : entry.getValue()) {
                if (fragment.hidden.contains(pref.getKey()) || pref.getKey().startsWith(StreamSettings.MORE_PREFIX)) continue;
                if (SettingsSearch.matches(query, pref.getTitle(), pref.getSummary(),
                        results.getTitle() + " " + java.util.Objects.toString(fragment.descriptions.get(pref.getKey()), ""))) {
                    // Search rows navigate to the original setting so dependency and dialog behavior stay intact.
                    Preference result = new Preference(activity);
                    result.setKey("search_result_" + pref.getKey());
                    result.setTitle(pref.getTitle());
                    result.setSummary(pref.getSummary());
                    if (fragment.idle.contains(pref.getKey()) || SettingRules.NOTE_ROWS.contains(pref.getKey())) {
                        // A row that does nothing now says why, in one line of its own: one the list leaves out, or
                        // one always listed whose summary (behind a closed More row) doesn't say it yet.
                        String reason = SettingRules.whenReason(activity, pref.getKey(),
                                fragment.getPreferenceManager().getSharedPreferences(), activity.devices());
                        CharSequence summary = pref.getSummary();
                        if (reason != null && (summary == null || !summary.toString().contains(reason))) {
                            result.setSummary(TextUtils.isEmpty(summary) ? reason : summary + "\n" + reason);
                        }
                    }
                    result.setOnPreferenceClickListener(clicked -> {
                        ((StreamSettings) fragment.getActivity()).openSearchResult(resultSection, pref.getKey());
                        return true;
                    });
                    results.addPreference(result);
                    resultCount++;
                }
            }
            if (results.getPreferenceCount() == 0) screen.removePreference(results);
        }
        if (screen.getPreferenceCount() == 0) {
            Preference empty = new Preference(activity);
            empty.setTitle(R.string.settings_search_empty);
            empty.setSummary(R.string.settings_search_empty_help);
            empty.setSelectable(false);
            empty.setLayoutResource(R.layout.settings_preference);
            screen.addPreference(empty);
        }
        StreamSettingsChrome.stylePreferences(screen, activity);
        fragment.setPreferenceScreen(screen);
        fragment.displayedResultCount = resultCount;
        TextView label = activity.contextLabel;
        if (!query.isEmpty() && label != null) {
            label.setText(fragment.getResources().getQuantityString(
                    R.plurals.settings_search_count, resultCount, resultCount));
        }
    }

    /**
     * A row of SettingRules.WHEN_ROWS in the list while it can do nothing (it holds a changed value, or a search result
     * opened it) says why: a switch in its summary, a list or slider after its value, as unusedReason's rows do.
     */
    static void showWhenReasons(StreamSettingsFragment fragment, SharedPreferences prefs) {
        StreamSettings activity = (StreamSettings) fragment.getActivity();
        if (activity == null) return;
        List<String> rows = new ArrayList<>(SettingRules.WHEN_ROWS);
        rows.addAll(SettingRules.NOTE_ROWS);
        for (String key : rows) {
            Preference pref = fragment.allPreferences.get(key);
            if (pref == null || SettingRules.TAGGED_SWITCHES.contains(key) || fragment.hidden.contains(key)
                    || fragment.idle.contains(key) || fragment.folded.contains(key)) {
                continue;
            }
            String reason = SettingRules.whenReason(activity, key, prefs, activity.devices());
            if (pref instanceof android.preference.TwoStatePreference) {
                pref.setSummary(reason);
            } else if (SettingRules.IGDB_SECRET.equals(key)) {
                String help = fragment.getString(R.string.game_info_igdb_secret_summary);
                pref.setSummary(reason == null ? help : help + "\n" + reason);
            } else if (reason != null) {
                CharSequence value = pref.getSummary();
                String summary = TextUtils.isEmpty(value) ? reason : fragment.getString(R.string.settings_joined, value, reason);
                // A list formats its summary (updateValueSummaries).
                pref.setSummary(pref instanceof ListPreference ? summary.replace("%", "%%") : summary);
            }
        }
    }

    /** Which profile streams use: none, the one in use, or the network's choice. */
    static String profilesSummary(Context context) {
        StreamProfiles.Snapshot profiles = StreamProfiles.load(context);
        String summary = profiles.exists(profiles.activeId) ?
                context.getString(R.string.profiles_summary_in_use, profiles.name(profiles.activeId)) :
                context.getString(R.string.profiles_summary_none);
        return profiles.auto ? summary + "\n" + context.getString(R.string.profiles_summary_auto) : summary;
    }

    static void updateValueSummaries(StreamSettingsFragment fragment, PreferenceGroup group) {
        Activity activity = fragment.getActivity();
        SharedPreferences prefs = fragment.getPreferenceManager().getSharedPreferences();
        for (int i = 0; i < group.getPreferenceCount(); i++) {
            Preference pref = group.getPreference(i);
            if (pref instanceof PreferenceGroup) {
                updateValueSummaries(fragment, (PreferenceGroup) pref);
                continue;
            }
            CharSequence value;
            if (pref instanceof ListPreference) {
                value = ((ListPreference) pref).getEntry();
                if (pref instanceof LanguagePreference && Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    android.os.LocaleList locales = activity.getSystemService(android.app.LocaleManager.class).getApplicationLocales();
                    if (!locales.isEmpty()) value = locales.get(0).getDisplayName();
                }
            } else if (pref instanceof SeekBarPreference) {
                value = ((SeekBarPreference) pref).getValueText();
            } else if (TouchMode.PREF.equals(pref.getKey())) {
                int mode = storedTouchMode(prefs);
                value = fragment.getResources().getStringArray(R.array.stream_touch_modes)[mode];
            } else if (StreamSettings.SHARP_COLOR_KEY.equals(pref.getKey())) {
                value = fragment.getResources().getStringArray(R.array.sharp_color_names)[
                        PreferenceConfiguration.sharpColor(prefs) ? 1 : 0];
            } else if (SwitchPairChoice.forKey(pref.getKey()) != null) {
                SwitchPairChoice pair = SwitchPairChoice.forKey(pref.getKey());
                int choice = pair.read(prefs);
                value = fragment.getResources().getStringArray(pair.names)[choice];
            } else if (StreamSettings.CONTROLLERS_KEY.equals(pref.getKey())) {
                value = fragment.getResources().getStringArray(R.array.controllers_names)[ControllersChoice.read(prefs)];
            } else if (StreamSettings.MOUSE_MODE_KEY.equals(pref.getKey())) {
                value = fragment.getResources().getStringArray(R.array.mouse_mode_names)[
                        prefs.getBoolean(StreamSettings.MOUSE_MODE_KEY, false) ? 1 : 0];
            } else if (com.limelight.console.OpenPlay.SETTING.equals(pref.getKey())) {
                value = startScreenName(activity, StartScreenChoice.read(prefs));
            } else if (pref instanceof ConsoleModePreference) {
                // The switch follows the start screen the row above it sets, without writing.
                ((ConsoleModePreference) pref).followStored();
                continue;
            } else if (StreamSettings.HOST_DISPLAY_KEY.equals(pref.getKey())) {
                value = DisplayArrangementChoice.names(activity, true)[DisplayArrangementChoice.readMain(prefs)];
            } else if (StreamSettings.PACE_CALIBRATION_KEY.equals(pref.getKey())) {
                continue;
            } else if (VideoScaling.PREF.equals(pref.getKey())) {
                value = fragment.getResources().getStringArray(R.array.video_scaling_modes)[storedVideoScaling(prefs)];
            } else if (OverlayChoice.ENABLED_KEY.equals(pref.getKey())) {
                value = fragment.getResources().getStringArray(R.array.overlay_choice_names)[OverlayChoice.read(prefs)];
            } else if (AppTheme.THEME.equals(pref.getKey())) {
                value = fragment.getResources().getStringArray(R.array.theme_choice_names)[ThemeChoice.read(prefs)];
                // High contrast is always black, whatever the picker says.
                if (prefs.getBoolean(AppTheme.HIGH_CONTRAST, false)) {
                    value = value + "\n" + fragment.getString(R.string.theme_high_contrast_note);
                }
            } else if (StreamSettings.LOW_LATENCY_CUSTOMIZE_KEY.equals(pref.getKey())) {
                value = fragment.getString(fragment.customizeOpen() ? R.string.low_latency_customize_open : R.string.low_latency_customize_closed);
            } else if (Arrays.asList(PreferenceConfiguration.LOW_LATENCY_DECODING_KEYS).contains(pref.getKey())) {
                // The master switch and profiles write preferences without going through these views.
                ((SwitchPreference) pref).setChecked(prefs.getBoolean(pref.getKey(), true));
                continue;
            } else if (StreamSettings.LOW_LATENCY_DECODING_KEY.equals(pref.getKey())) {
                // On only when every part is on; individual switches below can partially disable it.
                int on = PreferenceConfiguration.lowLatencyDecodingCount(prefs);
                ((SwitchPreference) pref).setChecked(on == PreferenceConfiguration.LOW_LATENCY_DECODING_KEYS.length);
                boolean partial = on > 0 && (on < PreferenceConfiguration.LOW_LATENCY_DECODING_KEYS.length ||
                        prefs.getBoolean(PreferenceConfiguration.DISABLE_HEVC_LOW_LATENCY_PREF_STRING, false));
                pref.setSummary(partial ? fragment.getString(R.string.settings_low_latency_partial) : null);
                continue;
            } else if (StreamSettings.AUTOMATIC_BITRATE_KEY.equals(pref.getKey())) {
                // Off once the slider is moved; profiles and presets write the flag without this view.
                ((SwitchPreference) pref).setChecked(!PreferenceConfiguration.isBitrateUserSet(prefs));
                continue;
            } else if (SettingRules.TAGGED_SWITCHES.contains(pref.getKey())) {
                // A switch has no value line; where it does nothing with the other settings it says why.
                pref.setSummary(SettingRules.unusedReason(activity, pref.getKey(), prefs));
                continue;
            } else if ("checkbox_display_queue_recovery".equals(pref.getKey())) {
                // A switch has no value line; where the take-back does nothing it says why.
                pref.setSummary(SettingRules.displayQueueRecoveryUnused(activity, prefs));
                continue;
            } else if (SettingRules.micEffect(pref.getKey()) != null && !(pref instanceof ListPreference)) {
                // The switches have no value line; it says when the device lacks the effect.
                pref.setSummary(SettingRules.micEffectMissing(pref.getKey(), prefs, fragment.micEffects()) ?
                        fragment.getString(R.string.mic_effect_not_on_device) : null);
                continue;
            } else if ("gpu_driver".equals(pref.getKey())) {
                GpuDriverStore.Driver driver = GpuDriverStore.selected(activity);
                value = driver == null ? fragment.getString(R.string.gpu_driver_system) : driver.name;
            } else {
                continue;
            }
            if (PreferenceConfiguration.UPSCALING_PREF_STRING.equals(pref.getKey()) &&
                    !"off".equals(((ListPreference) pref).getValue())) {
                // Say in the value line when the chosen upscaler is not the one used.
                if (!SettingRules.upscalingApplies(prefs)) {
                    value = fragment.getString(R.string.settings_joined, value, fragment.getString(R.string.upscaling_vulkan_short));
                } else if (prefs.getBoolean("checkbox_enable_hdr", false)) {
                    value = fragment.getString(R.string.settings_joined, value, fragment.getString(R.string.upscaling_hdr_short));
                } else if ("forcepyrowave".equals(prefs.getString("video_format", "auto"))) {
                    value = fragment.getString(R.string.settings_joined, value, fragment.getString(R.string.upscaling_pyrowave_short));
                } else if (prefs.getBoolean("checkbox_enable_sdr_10bit", false) &&
                        // Only when 10-bit SDR is not refused by settings alone (HDR and PyroWave are above).
                        // Sharper colored text gives way per PC (NvConnection), so it does not count here.
                        com.limelight.binding.video.Sdr10Policy.refusal(true, true, false, true, true, false, false) ==
                                com.limelight.binding.video.Sdr10Policy.Refusal.NONE) {
                    value = fragment.getString(R.string.settings_joined, value, fragment.getString(R.string.upscaling_tenbit_short));
                }
            }
            if (value != null && !SettingRules.TAGGED_SWITCHES.contains(pref.getKey())) {
                String unused = SettingRules.unusedReason(activity, pref.getKey(), prefs);
                if (unused != null) value = fragment.getString(R.string.settings_joined, value, unused);
            }
            if (value != null && SettingRules.FRAME_PACING.equals(pref.getKey())) {
                // A choice the renderer ignores says so in the value line, as Upscaling does.
                String unused = SettingRules.framePacingUnused(activity, prefs);
                if (unused != null) value = fragment.getString(R.string.settings_joined, value, unused);
            }
            if (value != null && PreferenceConfiguration.BITRATE_PREF_STRING.equals(pref.getKey())) {
                // A chosen bitrate below PyroWave's minimum stays; the value line says what it costs.
                // Measured at the size the stream gets, also when the resolution follows the screen.
                PreferenceConfiguration config = PreferenceConfiguration.readPreferences(activity);
                String warning = PreferenceConfiguration.pyroWaveBelowMinimumWarning(activity, config);
                if (warning != null) value = value + "\n" + warning;
            }
            if (value != null && SettingRules.micEffectMissing(pref.getKey(), prefs, fragment.micEffects())) {
                value = fragment.getString(R.string.settings_joined, value, fragment.getString(R.string.mic_effect_not_on_device));
            }
            if (value != null) {
                pref.setSummary(pref instanceof ListPreference ? value.toString().replace("%", "%%") : value);
            }
        }
    }

    static void updateCodecSummary(StreamSettingsFragment fragment, ListPreference codec, String value, boolean hdr) {
        final int generation = ++fragment.codecSummaryGeneration;
        if ("forcepyrowave".equals(value)) {
            Activity activity = fragment.getActivity();
            setCodecSummary(fragment, codec, fragment.getString(R.string.pyrowave_checking));
            new Thread(() -> {
                int summary = PyroWaveDecoderRenderer.getReadinessSummary(activity.getApplicationContext(), hdr);
                activity.runOnUiThread(() -> {
                    if (fragment.isAdded() && fragment.getActivity() == activity && !activity.isFinishing() &&
                            generation == fragment.codecSummaryGeneration) {
                        setCodecSummary(fragment, codec, fragment.getString(summary));
                    }
                });
            }, "GPU capability check").start();
            return;
        }
        CharSequence help = fragment.getString(R.string.summary_video_format);
        String measured = DecoderBenchmark.summary(fragment.getActivity());
        if (measured != null) {
            help = help + "\n\n" + fragment.getString(R.string.video_format_measured, measured);
        }
        setCodecSummary(fragment, codec, help);
    }

    static void setCodecSummary(StreamSettingsFragment fragment, ListPreference codec, CharSequence help) {
        fragment.descriptions.put("video_format", help);
        codec.setDialogMessage(help);
    }

    static int storedTouchMode(SharedPreferences prefs) {
        return TouchMode.resolve(prefs.getString(TouchMode.PREF, null),
                prefs.getBoolean(TouchMode.TRACKPAD_PREF, true), prefs.getBoolean(TouchMode.NATIVE_TOUCH_PREF, false));
    }

    static int storedVideoScaling(SharedPreferences prefs) {
        return VideoScaling.resolve(prefs.getString(VideoScaling.PREF, null),
                prefs.getBoolean(VideoScaling.STRETCH_PREF, false));
    }

    /** A start screen's entry: Home, Show my PCs, Continue my last game, or the chosen game's title. */
    static String startScreenName(Context context, String screen) {
        if (StartScreenChoice.CHOSEN.equals(screen)) {
            String title = com.limelight.console.OpenPlay.chosenTitle(context);
            if (title != null) return title;
        }
        return context.getResources().getStringArray(R.array.start_screen_names)[StartScreenChoice.index(screen)];
    }
}

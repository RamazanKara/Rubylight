package com.limelight.preferences;

import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.os.Bundle;
import android.preference.Preference;
import android.preference.PreferenceFragment;
import android.preference.PreferenceManager;
import android.preference.PreferenceScreen;
import android.view.LayoutInflater;
import android.view.View;
import android.view.ViewGroup;
import android.widget.ListView;

import com.limelight.BuildConfig;
import com.limelight.R;
import com.limelight.ui.GlassForm;

import java.util.HashMap;
import java.util.Locale;
import java.util.Map;

/**
 * Overview preference fragment listing {@link StreamSettings} sections, live summaries, and
 * rail fold state. Subclassed by {@link StreamSettings.RootFragment} to preserve the nested
 * fragment class name.
 */
public abstract class SettingsRootFragment extends PreferenceFragment {
    private final SharedPreferences.OnSharedPreferenceChangeListener summaryUpdater = (prefs, key) -> {
        updateSummaries();
        // Show debug settings, or a Debug setting changed or set back, adds or removes Debug.
        applyFold();
    };
    /** Every row of the overview by key, in the list or folded away. */
    private final Map<String, Preference> rows = new HashMap<>();

    @Override
    public Preference findPreference(CharSequence key) {
        Preference row = super.findPreference(key);
        return row != null ? row : rows.get(String.valueOf(key));
    }

    /** Shows or removes the rows behind More settings, as the activity has it, and points the row's icon. */
    void applyFold() {
        PreferenceScreen screen = getPreferenceScreen();
        StreamSettings activity = (StreamSettings) getActivity();
        if (screen == null || activity == null) return;
        boolean open = activity.railOpen();
        for (String key : StreamSettings.RAIL_FOLDED) {
            Preference row = rows.get(key);
            if (row == null) continue;
            boolean listed = screen.findPreference(key) != null;
            // Debug only on request (Show debug settings), while one of its settings is changed, or while it shows.
            boolean show = open && (!"debug".equals(key) || activity.debugListed());
            if (show && !listed) screen.addPreference(row);
            else if (!show && listed) screen.removePreference(row);
        }
        Preference more = rows.get(StreamSettings.RAIL_MORE_KEY);
        if (more instanceof FoldPreference) ((FoldPreference) more).setExpanded(open);
    }

    private void updateSummaries() {
        Context context = getActivity();
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(context);
        String bitrate = config.bitrate % 1000 == 0 ? Integer.toString(config.bitrate / 1000) :
                String.format(Locale.getDefault(), "%.1f", config.bitrate / 1000f);
        findPreference("video").setSummary(getString(R.string.settings_section_video_summary,
                config.width * 9 == config.height * 16 ? config.height + "p" : config.width + "×" + config.height,
                config.fps + " " + getString(R.string.fps_suffix_fps),
                bitrate + " " + getString(R.string.suffix_seekbar_bitrate_mbps),
                StreamSettings.entryFor(context, R.array.video_format_names, R.array.video_format_values,
                        prefs.getString(PreferenceConfiguration.VIDEO_FORMAT_PREF_STRING,
                                PreferenceConfiguration.DEFAULT_VIDEO_FORMAT))));
        String upscaling = prefs.getString(PreferenceConfiguration.UPSCALING_PREF_STRING, "off");
        findPreference("display").setSummary(getString(R.string.settings_joined,
                config.vrr ? getString(R.string.settings_display_adaptive) :
                        StreamSettings.entryFor(context, R.array.frame_pacing_choice_names, R.array.frame_pacing_choice_values,
                                FramePacingChoice.read(prefs)),
                "off".equals(upscaling) ? getString(R.string.settings_display_no_upscaling) :
                        StreamSettings.entryFor(context, R.array.upscaling_names, R.array.upscaling_values, upscaling)));
        findPreference("audio").setSummary(StreamSettings.entryFor(context, R.array.audio_config_names, R.array.audio_config_values,
                prefs.getString(PreferenceConfiguration.AUDIO_CONFIG_PREF_STRING, "2")));
        findPreference("app").setSummary(getString(R.string.settings_section_app_summary, BuildConfig.VERSION_NAME));
        findPreference(StreamSettings.PROFILES_KEY).setSummary(StreamSettings.SettingsFragment.profilesSummary(context));
    }

    @Override
    public void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        // One order on every screen: a TV opens on Controls in its place, not with Controls moved to the rail's top.
        addPreferencesFromResource(R.xml.preferences_root);
        StreamSettingsChrome.stylePreferences(getPreferenceScreen(), getActivity());
        for (int i = 0; i < getPreferenceScreen().getPreferenceCount(); i++) {
            Preference row = getPreferenceScreen().getPreference(i);
            // A row taken out keeps its place for when it comes back.
            row.setOrder(i);
            rows.put(row.getKey(), row);
        }
        Preference more = findPreference(StreamSettings.RAIL_MORE_KEY);
        more.setLayoutResource(GlassForm.of(getActivity()) == GlassForm.TELEVISION ? R.layout.settings_section_tv_fold : R.layout.settings_section_fold);
        more.setOnPreferenceClickListener(preference -> {
            ((StreamSettings) getActivity()).toggleRail();
            return true;
        });
        applyFold();
        for (String key : StreamSettings.SECTIONS.keySet()) {
            Preference row = findPreference(key);
            if (row == null) continue;
            row.setLayoutResource(StreamSettingsChrome.sectionLayout(getActivity()));
            row.setIcon(SettingsRail.iconOf(key));
            row.setOnPreferenceClickListener(preference -> {
                ListView list = (ListView) getView().findViewById(android.R.id.list);
                ((StreamSettings) getActivity()).activateSection(key, list.hasFocus());
                return true;
            });
        }
        // Profiles opens its own screen, with a section's look: a title, what is in use, a chevron.
        Preference profiles = findPreference(StreamSettings.PROFILES_KEY);
        profiles.setLayoutResource(StreamSettingsChrome.sectionLayout(getActivity()));
        profiles.setIcon(SettingsRail.iconOf(StreamSettings.PROFILES_KEY));
        profiles.setOnPreferenceClickListener(preference -> {
            startActivity(new Intent(getActivity(), StreamProfilesActivity.class));
            return true;
        });
    }

    @Override
    public View onCreateView(LayoutInflater inflater, ViewGroup container, Bundle savedInstanceState) {
        View view = super.onCreateView(inflater, container, savedInstanceState);
        StreamSettings activity = (StreamSettings) getActivity();
        StreamSettingsChrome.stylePreferenceList(view, activity, false);
        if (activity.twoPane) {
            ListView list = view.findViewById(android.R.id.list);
            list.setChoiceMode(ListView.CHOICE_MODE_SINGLE);
            // The right pane follows the row the D-pad is on.
            list.setOnItemSelectedListener(new android.widget.AdapterView.OnItemSelectedListener() {
                @Override
                public void onItemSelected(android.widget.AdapterView<?> parent, View row, int position, long id) {
                    Object item = parent.getItemAtPosition(position);
                    if (list.hasFocus() && item instanceof Preference) {
                        activity.sectionRowSelected(((Preference) item).getKey());
                    }
                }

                @Override
                public void onNothingSelected(android.widget.AdapterView<?> parent) { }
            });
            list.setOnFocusChangeListener((focused, hasFocus) -> activity.markSection());
        }
        return view;
    }

    @Override
    public void onActivityCreated(Bundle savedInstanceState) {
        super.onActivityCreated(savedInstanceState);
        StreamSettings activity = (StreamSettings) getActivity();
        ListView list = (ListView) getView().findViewById(android.R.id.list);
        android.widget.AdapterView.OnItemClickListener inner = list.getOnItemClickListener();
        if (!activity.twoPane || inner == null) return;
        // The list marks whatever row was clicked; the highlight belongs to the section shown on the right.
        list.setOnItemClickListener((parent, row, position, id) -> {
            inner.onItemClick(parent, row, position, id);
            activity.markSection();
        });
    }

    static int positionOf(ListView list, String key) {
        for (int i = 0; key != null && i < list.getCount(); i++) {
            Object item = list.getItemAtPosition(i);
            if (item instanceof Preference && key.equals(((Preference) item).getKey())) return i;
        }
        return -1;
    }

    /** Highlights the row of the section shown in the other pane; a null section highlights none. */
    void mark(String section) {
        ListView list = getView() == null ? null : (ListView) getView().findViewById(android.R.id.list);
        if (list == null || list.getAdapter() == null) return;
        int position = positionOf(list, section);
        int checked = list.getCheckedItemPosition();
        if (position >= 0 && position != checked) list.setItemChecked(position, true);
        else if (section == null && checked >= 0) list.setItemChecked(checked, false);
    }

    @Override
    public void onResume() {
        super.onResume();
        getPreferenceManager().getSharedPreferences().registerOnSharedPreferenceChangeListener(summaryUpdater);
        updateSummaries();
        StreamSettings activity = (StreamSettings) getActivity();
        if (activity.twoPane) {
            activity.markSection();
        } else {
            activity.searchResultKey = null;
        }
    }

    @Override
    public void onPause() {
        getPreferenceManager().getSharedPreferences().unregisterOnSharedPreferenceChangeListener(summaryUpdater);
        super.onPause();
    }
}

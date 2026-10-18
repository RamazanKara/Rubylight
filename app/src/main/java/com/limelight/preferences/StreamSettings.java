package com.limelight.preferences;

import android.Manifest;
import android.annotation.SuppressLint;
import android.content.Context;
import android.content.ActivityNotFoundException;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.content.res.Configuration;
import android.media.MediaCodecInfo;
import android.os.Build;
import android.os.Bundle;
import androidx.appcompat.app.AlertDialog;
import com.limelight.ui.NightjarActivity;
import androidx.appcompat.widget.SearchView;
import androidx.activity.OnBackPressedCallback;
import android.os.Handler;
import android.os.Vibrator;
import android.preference.SwitchPreference;
import android.preference.ListPreference;
import android.preference.Preference;
import android.preference.PreferenceCategory;
import android.preference.PreferenceFragment;
import android.preference.PreferenceGroup;
import android.preference.PreferenceManager;
import android.preference.PreferenceScreen;
import android.text.TextUtils;
import android.util.DisplayMetrics;
import android.util.Range;
import android.view.Display;
import android.view.DisplayCutout;
import android.view.LayoutInflater;
import android.view.View;
import android.view.ViewGroup;
import android.view.WindowInsets;
import android.widget.Toast;
import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import android.content.res.XmlResourceParser;
import android.widget.TextView;
import android.widget.ListView;
import com.limelight.update.UpdateFlow;
import com.limelight.update.UpdatePrefs;
import com.limelight.utils.HelpLauncher;
import org.xmlpull.v1.XmlPullParser;
import org.xmlpull.v1.XmlPullParserException;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;

import com.limelight.BuildConfig;
import com.limelight.binding.input.touch.TouchMode;
import com.limelight.LimeLog;
import com.limelight.PcView;
import com.limelight.R;
import com.limelight.binding.video.GpuDriverStore;
import com.limelight.binding.video.MediaCodecHelper;
import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.binding.video.DecoderBenchmark;
import com.limelight.binding.video.PyroWaveDecoderRenderer;
import com.limelight.utils.Dialog;
import com.limelight.utils.UiHelper;
import com.limelight.ui.TopAppBar;
import com.limelight.ui.StreamOrientation;
import com.limelight.ui.VideoScaling;
import com.google.android.material.appbar.MaterialToolbar;

import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.OutputStream;
import java.util.Arrays;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.Map;

public class StreamSettings extends NightjarActivity {
    private static final int EXPORT_LATENCY_REQUEST = 1;
    private static final String STATE_SECTION = "section";
    static final String LOW_LATENCY_DECODING_KEY = "low_latency_decoding";
    // Top-level screen keys (preferences_root.xml) and the preference categories each one shows
    static final Map<String, String[]> SECTIONS = new LinkedHashMap<>();
    static {
        SECTIONS.put("video", new String[] {"category_basic_settings"});
        SECTIONS.put("display", new String[] {"category_display_refresh", "category_upscaling", "category_host_display"});
        SECTIONS.put("input", new String[] {"category_gamepad_settings", "category_usb_controllers", "category_input_settings",
                "category_onscreen_controls", "category_keyboard_shortcuts"});
        SECTIONS.put("audio", new String[] {"category_audio_settings"});
        SECTIONS.put("streaming", new String[] {"category_streaming"});
        SECTIONS.put("app", new String[] {"category_ui_settings"});
        SECTIONS.put("advanced", new String[] {"category_advanced_settings", "category_advanced_decoder",
                "category_advanced_input", "category_advanced_diagnostics"});
    }
    // Kept by a reset: the language is applied by the OS, the others are actions
    private static final List<String> RESET_EXCLUDED_KEYS = Arrays.asList(
            PreferenceConfiguration.LANGUAGE_PREF_STRING, "export_latency_csv", "about_app", "check_updates_now", "gpu_driver");

    private PreferenceConfiguration previousPrefs;
    private int previousDisplayPixelCount;
    private String section;
    private MaterialToolbar toolbar;
    private String searchQuery = "";
    private SearchView searchView;
    private String searchResultKey;
    private String returnSearchQuery;
    private String returnSearchSection;
    private TextView contextLabel;
    private boolean changingQuery;
    private boolean stateSaved;
    private boolean renderPending;
    private boolean searchPending;
    private final Handler navigationHandler = new Handler();
    private final Runnable searchUpdate = this::renderSearch;

    /** Whether custom GPU drivers can load here. The debug build's demo mode shows their screen anyway. */
    protected boolean supportsGpuDrivers() {
        return GpuDriverStore.isSupported();
    }

    /** Whether USB controllers can connect here. The debug build's demo mode shows their settings anyway. */
    protected boolean supportsUsbHost() {
        return getPackageManager().hasSystemFeature(PackageManager.FEATURE_USB_HOST);
    }

    void reloadSettings() {
        navigationHandler.removeCallbacks(searchUpdate);
        searchPending = false;
        if (isFinishing() || isDestroyed()) return;
        if (stateSaved) {
            renderPending = true;
            return;
        }
        section = normalizeSection(section);
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            Display.Mode mode = getWindowManager().getDefaultDisplay().getMode();
            previousDisplayPixelCount = mode.getPhysicalWidth() * mode.getPhysicalHeight();
        }
        getFragmentManager().beginTransaction().replace(
                R.id.stream_settings, section == null && searchQuery.isEmpty() ?
                        new RootFragment() : SettingsFragment.forSection(section)
        ).commit();
        updateHeader();
    }

    private void updateHeader() {
        toolbar.setTitle(!searchQuery.isEmpty() ? R.string.settings_search_results :
                section == null ? R.string.settings : sectionTitle(section));
        if (searchQuery.isEmpty()) {
            contextLabel.setText(section == null ? R.string.settings_global_context : R.string.settings_changes_saved);
        } else {
            android.app.Fragment fragment = getFragmentManager().findFragmentById(R.id.stream_settings);
            if (fragment instanceof SettingsFragment) {
                int count = ((SettingsFragment) fragment).displayedResultCount;
                contextLabel.setText(getResources().getQuantityString(R.plurals.settings_search_count, count, count));
            }
        }
        toolbar.setNavigationContentDescription(returnSearchQuery != null ? R.string.settings_back_to_search :
                section == null ? R.string.settings_navigate_up : R.string.settings_back_to_overview);
    }

    private void renderSearch() {
        searchPending = false;
        if (isFinishing() || isDestroyed()) return;
        if (stateSaved) {
            renderPending = true;
            return;
        }
        android.app.Fragment fragment = getFragmentManager().findFragmentById(R.id.stream_settings);
        if (fragment instanceof SettingsFragment && !searchQuery.isEmpty()) {
            ((SettingsFragment) fragment).filterPreferences();
            updateHeader();
        } else {
            reloadSettings();
        }
    }

    private void setSearchQuery(String query) {
        changingQuery = true;
        searchQuery = query;
        searchView.setQuery(query, false);
        changingQuery = false;
        searchView.clearFocus();
        searchView.setVisibility(query.isEmpty() ? View.GONE : View.VISIBLE);
        reloadSettings();
    }

    /** Shows the search field under the top app bar and focuses it; search stays out of the way until asked for. */
    public void openSearch() {
        searchView.setVisibility(View.VISIBLE);
        searchView.setIconified(false);
        searchView.requestFocus();
    }

    private void openSearchResult(String targetSection, String key) {
        returnSearchQuery = searchQuery;
        returnSearchSection = section;
        section = targetSection;
        searchResultKey = key;
        setSearchQuery("");
    }

    private static String normalizeSection(String section) {
        // Restore navigation saved by versions that placed these controls in a separate section.
        return "developer".equals(section) ? "advanced" : section;
    }

    static int sectionTitle(String section) {
        switch (section) {
            case "video": return R.string.settings_section_video;
            case "display": return R.string.settings_section_display;
            case "input": return R.string.settings_section_input;
            case "audio": return R.string.settings_section_audio;
            case "streaming": return R.string.settings_section_streaming;
            case "advanced": return R.string.settings_section_advanced;
            default: return R.string.settings_section_app;
        }
    }

    static String sectionOf(String category) {
        for (Map.Entry<String, String[]> item : SECTIONS.entrySet()) {
            if (Arrays.asList(item.getValue()).contains(category)) {
                return item.getKey();
            }
        }
        return null;
    }

    void showSection(String newSection) {
        section = newSection;
        reloadSettings();
    }

    private void navigateBack() {
        if (searchQuery.isEmpty() && searchView.getVisibility() == View.VISIBLE) {
            searchView.clearFocus();
            searchView.setVisibility(View.GONE);
        }
        else if (!searchQuery.isEmpty()) {
            setSearchQuery("");
        }
        else if (returnSearchQuery != null) {
            String query = returnSearchQuery;
            section = returnSearchSection;
            returnSearchQuery = null;
            returnSearchSection = null;
            setSearchQuery(query);
        }
        else if (section != null) {
            showSection(null);
        }
        else {
            finishSettings();
        }
    }

    // Removes every setting shown in these screens, then restores the XML defaults
    static void resetAllSettings(Context context) {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
        SharedPreferences.Editor editor = prefs.edit();
        try (XmlResourceParser parser = context.getResources().getXml(R.xml.preferences)) {
            for (int event = parser.getEventType(); event != XmlPullParser.END_DOCUMENT; event = parser.next()) {
                if (event == XmlPullParser.START_TAG) {
                    String key = parser.getAttributeValue("http://schemas.android.com/apk/res/android", "key");
                    if (key != null && !RESET_EXCLUDED_KEYS.contains(key)) {
                        editor.remove(key);
                    }
                }
            }
        } catch (XmlPullParserException | IOException e) {
            LimeLog.warning("Unable to read preference keys: " + e);
            return;
        }
        editor.remove("checkbox_native_touch")
                .remove(PreferenceConfiguration.BITRATE_USER_SET_PREF_STRING)
                .remove(PreferenceConfiguration.BITRATE_PREF_OLD_STRING)
                .remove(PreferenceConfiguration.PREFS_VERSION_PREF_STRING)
                .commit();
        PreferenceManager.setDefaultValues(context, R.xml.preferences, true);
        PreferenceConfiguration.migrateToNativeDefaults(context);
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        // Framework fragments can be recreated by super.onCreate(), before our views exist.
        if (savedInstanceState != null) {
            section = normalizeSection(savedInstanceState.getString(STATE_SECTION));
            searchQuery = savedInstanceState.getString("search", "");
            returnSearchQuery = savedInstanceState.getString("return_search");
            returnSearchSection = normalizeSection(savedInstanceState.getString("return_section"));
            searchResultKey = savedInstanceState.getString("search_result");
        }
        super.onCreate(savedInstanceState);

        previousPrefs = PreferenceConfiguration.readPreferences(this);

        UiHelper.setLocale(this);

        setContentView(R.layout.activity_stream_settings);
        toolbar = TopAppBar.configure(this, R.id.settings_toolbar, getString(R.string.settings), R.menu.settings);
        TopAppBar.setUpNavigation(toolbar, R.id.settings_back, v -> navigateBack());
        toolbar.setOnMenuItemClickListener(item -> {
            openSearch();
            return true;
        });
        contextLabel = findViewById(R.id.settings_context);
        searchView = findViewById(R.id.settings_search);
        searchView.setVisibility(searchQuery.isEmpty() ? View.GONE : View.VISIBLE);
        searchView.setIconifiedByDefault(false);
        searchView.setQueryHint(getString(R.string.settings_search_hint));
        searchView.setQuery(searchQuery, false);
        searchView.clearFocus();
        searchView.setOnQueryTextListener(new SearchView.OnQueryTextListener() {
            @Override public boolean onQueryTextSubmit(String query) {
                navigationHandler.removeCallbacks(searchUpdate);
                renderSearch();
                searchView.clearFocus();
                return true;
            }

            @Override public boolean onQueryTextChange(String query) {
                if (changingQuery) return true;
                searchQuery = query.trim();
                returnSearchQuery = null;
                returnSearchSection = null;
                navigationHandler.removeCallbacks(searchUpdate);
                searchPending = true;
                navigationHandler.postDelayed(searchUpdate, 120);
                return true;
            }
        });
        getOnBackPressedDispatcher().addCallback(this, new OnBackPressedCallback(true) {
            @Override public void handleOnBackPressed() { navigateBack(); }
        });
        updateHeader();

        UiHelper.notifyNewRootView(this);
    }

    @Override
    protected void onSaveInstanceState(Bundle outState) {
        // Save the screen for the latest query, including a last keystroke before rotation.
        navigationHandler.removeCallbacks(searchUpdate);
        if (searchPending) renderSearch();
        getFragmentManager().executePendingTransactions();
        stateSaved = true;
        super.onSaveInstanceState(outState);
        outState.putString(STATE_SECTION, section);
        outState.putString("search", searchQuery);
        outState.putString("return_search", returnSearchQuery);
        outState.putString("return_section", returnSearchSection);
        outState.putString("search_result", searchResultKey);
    }

    @Override
    protected void onPostResume() {
        super.onPostResume();
        stateSaved = false;
        if (renderPending) {
            renderPending = false;
            reloadSettings();
        }
    }

    @Override
    protected void onDestroy() {
        Dialog.closeDialogs(this);
        navigationHandler.removeCallbacksAndMessages(null);
        super.onDestroy();
    }

    private void exportLatencyCsv() {
        if (!new File(getFilesDir(), MediaCodecDecoderRenderer.LATENCY_CSV_NAME).isFile()) {
            Toast.makeText(this, R.string.latency_csv_empty, Toast.LENGTH_LONG).show();
            return;
        }
        Intent intent = new Intent(Intent.ACTION_CREATE_DOCUMENT);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        intent.setType("text/csv");
        intent.putExtra(Intent.EXTRA_TITLE, MediaCodecDecoderRenderer.LATENCY_CSV_EXPORT_NAME);
        try {
            startActivityForResult(intent, EXPORT_LATENCY_REQUEST);
        } catch (ActivityNotFoundException e) {
            Toast.makeText(this, R.string.latency_csv_no_picker, Toast.LENGTH_LONG).show();
        }
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode != EXPORT_LATENCY_REQUEST || resultCode != RESULT_OK || data == null || data.getData() == null) {
            return;
        }
        new Thread(new Runnable() {
            @Override
            public void run() {
                int message = R.string.latency_csv_saved;
                try (FileInputStream input = new FileInputStream(new File(getFilesDir(), MediaCodecDecoderRenderer.LATENCY_CSV_NAME));
                     OutputStream output = getContentResolver().openOutputStream(data.getData(), "wt")) {
                    if (output == null) {
                        throw new IOException("Document provider did not open the CSV");
                    }
                    byte[] buffer = new byte[16384];
                    int count;
                    while ((count = input.read(buffer)) != -1) {
                        output.write(buffer, 0, count);
                    }
                } catch (IOException | SecurityException e) {
                    LimeLog.warning("Unable to export latency CSV: " + e);
                    message = R.string.latency_csv_failed;
                }
                final int resultMessage = message;
                runOnUiThread(new Runnable() {
                    @Override
                    public void run() {
                        Toast.makeText(StreamSettings.this, resultMessage, Toast.LENGTH_LONG).show();
                    }
                });
            }
        }, "Latency CSV export").start();
    }

    @Override
    public void onAttachedToWindow() {
        super.onAttachedToWindow();

        if (getFragmentManager().findFragmentById(R.id.stream_settings) == null) {
            reloadSettings();
        } else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            Display.Mode mode = getWindowManager().getDefaultDisplay().getMode();
            previousDisplayPixelCount = mode.getPhysicalWidth() * mode.getPhysicalHeight();
        }
    }

    @Override
    public void onConfigurationChanged(Configuration newConfig) {
        super.onConfigurationChanged(newConfig);

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            Display.Mode mode = getWindowManager().getDefaultDisplay().getMode();

            // If the display's physical pixel count has changed, we consider that it's a new display
            // and we should reload our settings (which include display-dependent values).
            //
            // NB: We aren't using displayId here because that stays the same (DEFAULT_DISPLAY) when
            // switching between screens on a foldable device.
            if (mode.getPhysicalWidth() * mode.getPhysicalHeight() != previousDisplayPixelCount) {
                reloadSettings();
            }
        }
    }

    @SuppressLint("MissingSuperCall") // finish() is called below; super would only add the default back handling
    @Override
    // NOTE: This will NOT be called on Android 13+ with android:enableOnBackInvokedCallback="true"
    public void onBackPressed() {
        navigateBack();
    }

    private void finishSettings() {
        finish();

        // Language changes are handled via configuration changes in Android 13+,
        // so manual activity relaunching is no longer required.
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
            PreferenceConfiguration newPrefs = PreferenceConfiguration.readPreferences(this);
            if (!newPrefs.language.equals(previousPrefs.language)) {
                // Restart the PC view to apply UI changes
                Intent intent = new Intent(this, PcView.class);
                intent.setFlags(Intent.FLAG_ACTIVITY_CLEAR_TASK | Intent.FLAG_ACTIVITY_NEW_TASK);
                startActivity(intent, null);
            }
        }
    }

    private static void stylePreferences(PreferenceGroup group) {
        for (int i = 0; i < group.getPreferenceCount(); i++) {
            Preference preference = group.getPreference(i);
            if (preference instanceof PresetPreference) continue;
            if (preference instanceof PreferenceCategory) {
                // A group without a title continues the section heading above it.
                preference.setLayoutResource(TextUtils.isEmpty(preference.getTitle()) ?
                        R.layout.settings_category_spacer : R.layout.settings_category);
            } else {
                preference.setLayoutResource(R.layout.settings_preference);
            }
            if (preference instanceof PreferenceGroup) {
                stylePreferences((PreferenceGroup) preference);
            }
        }
    }

    private static void stylePreferenceList(View view, android.app.Activity activity) {
        ListView list = view.findViewById(android.R.id.list);
        com.limelight.ui.UiNavigation.bindPreferences(list);
        list.setNextFocusUpId(R.id.settings_search);
        View search = activity.findViewById(R.id.settings_search);
        if ((view.getResources().getConfiguration().uiMode & Configuration.UI_MODE_TYPE_MASK)
                == Configuration.UI_MODE_TYPE_TELEVISION && (search == null || !search.hasFocus())) {
            list.requestFocus();
        }
        list.setDivider(null);
        list.setDividerHeight(0);
        list.setSelector(R.drawable.list_focus_row);
        // Rows reach the edges and carry the screen gutter themselves (layout/settings_*.xml).
        list.setPadding(0, 0, 0, view.getResources().getDimensionPixelSize(R.dimen.space_l));
        list.setClipToPadding(false);
    }

    // "Automatic (recommended)" reads as "Automatic" in a one-line summary
    static CharSequence entryFor(Context context, int names, int values, String value) {
        String[] valueArray = context.getResources().getStringArray(values);
        for (int i = 0; i < valueArray.length; i++) {
            if (valueArray[i].equals(value)) {
                return context.getResources().getStringArray(names)[i].replaceFirst(" \\(.*\\)$", "");
            }
        }
        return value;
    }

    public static class RootFragment extends PreferenceFragment {
        private final SharedPreferences.OnSharedPreferenceChangeListener summaryUpdater = (prefs, key) -> updateSummaries();

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
                    entryFor(context, R.array.video_format_names, R.array.video_format_values,
                            prefs.getString(PreferenceConfiguration.VIDEO_FORMAT_PREF_STRING,
                                    PreferenceConfiguration.DEFAULT_VIDEO_FORMAT))));
            String upscaling = prefs.getString(PreferenceConfiguration.UPSCALING_PREF_STRING, "off");
            findPreference("display").setSummary(getString(R.string.settings_joined,
                    config.vrr ? getString(R.string.settings_display_adaptive) :
                            entryFor(context, R.array.video_frame_pacing_names, R.array.video_frame_pacing_values,
                                    prefs.getString(PreferenceConfiguration.FRAME_PACING_PREF_STRING,
                                            PreferenceConfiguration.DEFAULT_FRAME_PACING)),
                    "off".equals(upscaling) ? getString(R.string.settings_display_no_upscaling) :
                            entryFor(context, R.array.upscaling_names, R.array.upscaling_values, upscaling)));
            findPreference("audio").setSummary(entryFor(context, R.array.audio_config_names, R.array.audio_config_values,
                    prefs.getString(PreferenceConfiguration.AUDIO_CONFIG_PREF_STRING, "2")));
            findPreference("app").setSummary(getString(R.string.settings_section_app_summary, BuildConfig.VERSION_NAME));
        }

        @Override
        public void onCreate(Bundle savedInstanceState) {
            super.onCreate(savedInstanceState);
            addPreferencesFromResource(R.xml.preferences_root);
            if ((getResources().getConfiguration().uiMode & Configuration.UI_MODE_TYPE_MASK)
                    == Configuration.UI_MODE_TYPE_TELEVISION) {
                findPreference("input").setOrder(-1);
            }
            stylePreferences(getPreferenceScreen());
            for (String key : SECTIONS.keySet()) {
                Preference row = findPreference(key);
                if (row == null) continue;
                row.setLayoutResource(R.layout.settings_section);
                row.setOnPreferenceClickListener(preference -> {
                    ((StreamSettings) getActivity()).showSection(key);
                    return true;
                });
            }

        }

        @Override
        public View onCreateView(LayoutInflater inflater, ViewGroup container, Bundle savedInstanceState) {
            View view = super.onCreateView(inflater, container, savedInstanceState);
            stylePreferenceList(view, getActivity());
            return view;
        }

        @Override
        public void onResume() {
            super.onResume();
            getPreferenceManager().getSharedPreferences().registerOnSharedPreferenceChangeListener(summaryUpdater);
            updateSummaries();
            ((StreamSettings) getActivity()).searchResultKey = null;
        }

        @Override
        public void onPause() {
            getPreferenceManager().getSharedPreferences().unregisterOnSharedPreferenceChangeListener(summaryUpdater);
            super.onPause();
        }
    }

    public static class SettingsFragment extends PreferenceFragment {
        private static final String ARG_SECTION = "section";
        private static final int MIC_PERMISSION_REQUEST = 2;
        private final Map<PreferenceCategory, List<Preference>> searchCategories = new LinkedHashMap<>();
        private final Map<String, Preference> allPreferences = new HashMap<>();
        private int displayedResultCount;

        @Override
        public Preference findPreference(CharSequence key) {
            Preference preference = super.findPreference(key);
            return preference != null ? preference : allPreferences.get(key.toString());
        }

        static SettingsFragment forSection(String section) {
            SettingsFragment fragment = new SettingsFragment();
            Bundle args = new Bundle();
            args.putString(ARG_SECTION, section);
            fragment.setArguments(args);
            return fragment;
        }

        // Every category is built first so the device checks below can find their preferences
        private void filterPreferences() {
            PreferenceScreen screen = getPreferenceManager().createPreferenceScreen(getActivity());
            String query = ((StreamSettings) getActivity()).searchQuery;
            String section = normalizeSection(getArguments() == null ? null : getArguments().getString(ARG_SECTION));
            String[] categories = section == null ? null : SECTIONS.get(section);
            int resultCount = 0;
            if (!query.isEmpty() && SettingsSearch.matches(query, getString(R.string.stream_presets),
                    getString(R.string.stream_presets_help), String.join(" ", getResources().getStringArray(R.array.stream_preset_names)))) {
                Preference presets = new Preference(getActivity());
                presets.setTitle(R.string.stream_presets);
                presets.setOnPreferenceClickListener(pref -> {
                    StreamSettings activity = (StreamSettings) getActivity();
                    activity.openSearchResult(null, "stream_presets");
                    return true;
                });
                screen.addPreference(presets);
                resultCount++;
            }
            for (Map.Entry<PreferenceCategory, List<Preference>> entry : searchCategories.entrySet()) {
                PreferenceCategory category = entry.getKey();
                if (query.isEmpty()) {
                    if (categories == null || Arrays.asList(categories).contains(category.getKey())) screen.addPreference(category);
                    continue;
                }
                final String resultSection = sectionOf(category.getKey());
                PreferenceCategory results = new PreferenceCategory(getActivity());
                CharSequence sectionTitle = resultSection == null ? null : getString(sectionTitle(resultSection));
                results.setTitle(sectionTitle == null ? category.getTitle() :
                        TextUtils.isEmpty(category.getTitle()) ? sectionTitle :
                        getString(R.string.settings_search_breadcrumb, sectionTitle, category.getTitle()));
                screen.addPreference(results);
                for (Preference pref : entry.getValue()) {
                    if (SettingsSearch.matches(query, pref.getTitle(), pref.getSummary(),
                            results.getTitle() + " " + java.util.Objects.toString(descriptions.get(pref.getKey()), ""))) {
                        // Search rows navigate to the original setting so dependency and dialog behavior stay intact.
                        Preference result = new Preference(getActivity());
                        result.setKey("search_result_" + pref.getKey());
                        result.setTitle(pref.getTitle());
                        result.setSummary(pref.getSummary());
                        result.setOnPreferenceClickListener(clicked -> {
                            StreamSettings activity = (StreamSettings) getActivity();
                            activity.openSearchResult(resultSection, pref.getKey());
                            return true;
                        });
                        results.addPreference(result);
                        resultCount++;
                    }
                }
                if (results.getPreferenceCount() == 0) screen.removePreference(results);
            }
            if (screen.getPreferenceCount() == 0) {
                Preference empty = new Preference(getActivity());
                empty.setTitle(R.string.settings_search_empty);
                empty.setSummary(R.string.settings_search_empty_help);
                empty.setSelectable(false);
                empty.setLayoutResource(R.layout.settings_preference);
                screen.addPreference(empty);
            }
            stylePreferences(screen);
            setPreferenceScreen(screen);
            displayedResultCount = resultCount;
            TextView label = ((StreamSettings) getActivity()).contextLabel;
            if (!query.isEmpty() && label != null) {
                label.setText(getResources().getQuantityString(
                        R.plurals.settings_search_count, resultCount, resultCount));
            }
        }

        // Explanations that search matches besides the visible title and value
        private final Map<String, CharSequence> descriptions = new HashMap<>();
        private int codecSummaryGeneration;
        private final SharedPreferences.OnSharedPreferenceChangeListener summaryUpdater =
                (prefs, key) -> updateValueSummaries(getPreferenceScreen());

        /** Which profile streams use: none, the one in use, or the network's choice. */
        static String profilesSummary(Context context) {
            StreamProfiles.Snapshot profiles = StreamProfiles.load(context);
            String summary = profiles.exists(profiles.activeId) ?
                    context.getString(R.string.profiles_summary_in_use, profiles.name(profiles.activeId)) :
                    context.getString(R.string.profiles_summary_none);
            return profiles.auto ? summary + "\n" + context.getString(R.string.profiles_summary_auto) : summary;
        }

        private void updateValueSummaries(PreferenceGroup group) {
            SharedPreferences prefs = getPreferenceManager().getSharedPreferences();
            for (int i = 0; i < group.getPreferenceCount(); i++) {
                Preference pref = group.getPreference(i);
                if (pref instanceof PreferenceGroup) {
                    updateValueSummaries((PreferenceGroup) pref);
                    continue;
                }
                CharSequence value;
                if (pref instanceof ListPreference) {
                    value = ((ListPreference) pref).getEntry();
                    if (pref instanceof LanguagePreference && Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                        android.os.LocaleList locales = getActivity().getSystemService(android.app.LocaleManager.class).getApplicationLocales();
                        if (!locales.isEmpty()) value = locales.get(0).getDisplayName();
                    }
                } else if (pref instanceof SeekBarPreference) {
                    value = ((SeekBarPreference) pref).getValueText();
                } else if ("checkbox_touchscreen_trackpad".equals(pref.getKey())) {
                    int mode = storedTouchMode(prefs);
                    value = getResources().getStringArray(R.array.stream_touch_modes)[mode];
                } else if (VideoScaling.STRETCH_PREF.equals(pref.getKey())) {
                    value = getResources().getStringArray(R.array.video_scaling_modes)[storedVideoScaling(prefs)];
                } else if (Arrays.asList(PreferenceConfiguration.LOW_LATENCY_DECODING_KEYS).contains(pref.getKey())) {
                    // The master switch and profiles write preferences without going through these views.
                    ((SwitchPreference) pref).setChecked(prefs.getBoolean(pref.getKey(), true));
                    continue;
                } else if (LOW_LATENCY_DECODING_KEY.equals(pref.getKey())) {
                    // On only when every part is on; individual switches below can partially disable it.
                    int on = PreferenceConfiguration.lowLatencyDecodingCount(prefs);
                    ((SwitchPreference) pref).setChecked(on == PreferenceConfiguration.LOW_LATENCY_DECODING_KEYS.length);
                    boolean partial = on > 0 && (on < PreferenceConfiguration.LOW_LATENCY_DECODING_KEYS.length ||
                            prefs.getBoolean(PreferenceConfiguration.DISABLE_HEVC_LOW_LATENCY_PREF_STRING, false));
                    pref.setSummary(partial ? getString(R.string.settings_low_latency_partial) : null);
                    continue;
                } else if ("stream_profiles".equals(pref.getKey())) {
                    value = profilesSummary(getActivity());
                } else if ("gpu_driver".equals(pref.getKey())) {
                    GpuDriverStore.Driver driver = GpuDriverStore.selected(getActivity());
                    value = driver == null ? getString(R.string.gpu_driver_system) : driver.name;
                } else {
                    continue;
                }
                if (PreferenceConfiguration.UPSCALING_SHARPNESS_PREF_STRING.equals(pref.getKey())) {
                    String mode = prefs.getString(PreferenceConfiguration.UPSCALING_PREF_STRING, "off");
                    pref.setEnabled("fsr1".equals(mode) || "sgsr1".equals(mode));
                }
                if (PreferenceConfiguration.UPSCALING_PREF_STRING.equals(pref.getKey()) &&
                        !"off".equals(((ListPreference) pref).getValue())) {
                    // Say in the value line when the chosen upscaler is not the one used.
                    if (prefs.getBoolean("checkbox_enable_hdr", false)) {
                        value = getString(R.string.settings_joined, value, getString(R.string.upscaling_hdr_short));
                    } else if ("forcepyrowave".equals(prefs.getString("video_format", "auto"))) {
                        value = getString(R.string.settings_joined, value, getString(R.string.upscaling_pyrowave_short));
                    }
                }
                if ("adaptive_quality_min_resolution".equals(pref.getKey()) || "adaptive_quality_min_fps".equals(pref.getKey())) {
                    pref.setEnabled(!"off".equals(prefs.getString("adaptive_quality_mode", "off")));
                }
                if (value != null) {
                    pref.setSummary(pref instanceof ListPreference ? value.toString().replace("%", "%%") : value);
                }
            }
        }

        private boolean hasMicPermission() {
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M) {
                return true;
            }
            return getActivity().checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED;
        }

        private void showMicPermissionDenied() {
            new MaterialAlertDialogBuilder(getActivity())
                    .setTitle(R.string.title_checkbox_enable_mic)
                    .setMessage(R.string.mic_permission_denied)
                    .setPositiveButton(android.R.string.ok, null)
                    .show();
        }

        @Override
        public void onRequestPermissionsResult(int requestCode, String[] permissions, int[] grantResults) {
            if (requestCode != MIC_PERMISSION_REQUEST) {
                super.onRequestPermissionsResult(requestCode, permissions, grantResults);
                return;
            }
            boolean granted = grantResults.length > 0 && grantResults[0] == PackageManager.PERMISSION_GRANTED;
            SwitchPreference mic = (SwitchPreference) findPreference(PreferenceConfiguration.ENABLE_MIC_PREF_STRING);
            if (mic != null) {
                mic.setChecked(granted);
            }
            LimeLog.info("Microphone: permission " + (granted ? "granted" : "denied"));
            if (!granted && isAdded()) {
                showMicPermissionDenied();
            }
        }

        @Override
        public void onResume() {
            super.onResume();
            // Microphone access can be withdrawn in Android settings; the switch follows it.
            SwitchPreference mic = (SwitchPreference) findPreference(PreferenceConfiguration.ENABLE_MIC_PREF_STRING);
            if (mic != null && mic.isChecked() && !hasMicPermission()) {
                mic.setChecked(false);
                showMicPermissionDenied();
            }
            getPreferenceManager().getSharedPreferences().registerOnSharedPreferenceChangeListener(summaryUpdater);
            ListPreference codec = (ListPreference) findPreference("video_format");
            if (codec != null) {
                updateCodecSummary(codec, codec.getValue(), getPreferenceManager().getSharedPreferences()
                        .getBoolean("checkbox_enable_hdr", false));
            }
            updateValueSummaries(getPreferenceScreen());
            // The fragment is replaced after the window got its insets, so ask for them again
            // to keep the last settings clear of the navigation bar.
            if (getView() != null) {
                getView().requestApplyInsets();
                StreamSettings activity = (StreamSettings) getActivity();
                if (activity.searchResultKey != null) {
                    String key = activity.searchResultKey;
                    activity.searchResultKey = null;
                    ListView list = getView().findViewById(android.R.id.list);
                    list.post(() -> {
                        for (int i = 0; i < list.getCount(); i++) {
                            Object item = list.getItemAtPosition(i);
                            if (item instanceof Preference && key.equals(((Preference) item).getKey())) {
                                list.setSelection(i);
                                break;
                            }
                        }
                    });
                }
            }
        }

        @Override
        public void onPause() {
            getPreferenceManager().getSharedPreferences().unregisterOnSharedPreferenceChangeListener(summaryUpdater);
            super.onPause();
        }

        private void setValue(String preferenceKey, String value) {
            ListPreference pref = (ListPreference) findPreference(preferenceKey);

            pref.setValue(value);
        }

        private void appendPreferenceEntry(ListPreference pref, String newEntryName, String newEntryValue) {
            CharSequence[] newEntries = Arrays.copyOf(pref.getEntries(), pref.getEntries().length + 1);
            CharSequence[] newValues = Arrays.copyOf(pref.getEntryValues(), pref.getEntryValues().length + 1);

            // Add the new option
            newEntries[newEntries.length - 1] = newEntryName;
            newValues[newValues.length - 1] = newEntryValue;

            pref.setEntries(newEntries);
            pref.setEntryValues(newValues);
        }

        // A size taken from this screen: the panel itself, or the area beside its camera cutout.
        private void addNativeResolutionEntry(int nativeWidth, int nativeHeight, boolean avoidsCutout, boolean portrait) {
            ListPreference pref = (ListPreference) findPreference(PreferenceConfiguration.RESOLUTION_PREF_STRING);
            String newValue = nativeWidth + "x" + nativeHeight;

            // Check if the native resolution is already present
            for (CharSequence value : pref.getEntryValues()) {
                if (newValue.equals(value.toString())) {
                    // It is present in the default list, so don't add it again
                    return;
                }
            }

            String size = nativeWidth + "×" + nativeHeight;
            String panel = PreferenceConfiguration.nativeResolution(getActivity());
            String landscape = Math.max(nativeWidth, nativeHeight) + "x" + Math.min(nativeWidth, nativeHeight);
            String newName;
            if (avoidsCutout) {
                newName = getString(portrait ? R.string.resolution_screen_cutout_portrait : R.string.resolution_screen_cutout, size);
            } else if (panel == null || panel.equals(landscape)) {
                newName = getString(portrait ? R.string.resolution_screen_portrait : R.string.resolution_screen, size);
            } else {
                // Another mode the panel can switch to
                newName = size;
            }
            appendPreferenceEntry(pref, newName, newValue);
        }

        private void addNativeResolutionEntries(int nativeWidth, int nativeHeight, boolean avoidsCutout) {
            if (PreferenceConfiguration.isSquarishScreen(nativeWidth, nativeHeight)) {
                addNativeResolutionEntry(nativeHeight, nativeWidth, avoidsCutout, true);
            }
            addNativeResolutionEntry(nativeWidth, nativeHeight, avoidsCutout, false);
        }

        // Fixed sizes larger than the panel would only be scaled down again; they stay listed while chosen.
        static boolean fitsPanel(String size, String panel) {
            if (panel == null) return true;
            return PreferenceConfiguration.getWidthFromResolutionString(size) <= PreferenceConfiguration.getWidthFromResolutionString(panel) &&
                    PreferenceConfiguration.getHeightFromResolutionString(size) <= PreferenceConfiguration.getHeightFromResolutionString(panel);
        }

        // Largest first, so this screen's own size leads the list.
        private void sortResolutionEntries(boolean hideLargerThanPanel) {
            ListPreference pref = (ListPreference) findPreference(PreferenceConfiguration.RESOLUTION_PREF_STRING);
            String panel = PreferenceConfiguration.nativeResolution(getActivity());
            List<String[]> entries = new ArrayList<>();
            List<CharSequence> presets = Arrays.asList(getResources().getStringArray(R.array.resolution_values));
            for (int i = 0; i < pref.getEntryValues().length; i++) {
                String value = pref.getEntryValues()[i].toString();
                if (hideLargerThanPanel && presets.contains(value) && !fitsPanel(value, panel) && !value.equals(pref.getValue())) {
                    continue;
                }
                entries.add(new String[] {pref.getEntries()[i].toString(), value});
            }
            java.util.Collections.sort(entries, (a, b) -> Long.compare(pixels(b[1]), pixels(a[1])));
            CharSequence[] names = new CharSequence[entries.size()];
            CharSequence[] values = new CharSequence[entries.size()];
            for (int i = 0; i < entries.size(); i++) {
                names[i] = entries.get(i)[0];
                values[i] = entries.get(i)[1];
            }
            pref.setEntries(names);
            pref.setEntryValues(values);
        }

        private static long pixels(String size) {
            return (long) PreferenceConfiguration.getWidthFromResolutionString(size) *
                    PreferenceConfiguration.getHeightFromResolutionString(size);
        }

        private void addNativeFrameRateEntry(float framerate) {
            int frameRateRounded = Math.round(framerate);
            if (frameRateRounded == 0) {
                return;
            }

            ListPreference pref = (ListPreference) findPreference(PreferenceConfiguration.FPS_PREF_STRING);
            String fpsValue = Integer.toString(frameRateRounded);

            // Check if the native frame rate is already present
            for (CharSequence value : pref.getEntryValues()) {
                if (fpsValue.equals(value.toString())) {
                    // It is present in the default list, so don't add it again
                    return;
                }
            }

            appendPreferenceEntry(pref, getString(R.string.fps_screen, frameRateRounded), fpsValue);
        }

        private void removeValue(String preferenceKey, String value, Runnable onMatched) {
            int matchingCount = 0;

            ListPreference pref = (ListPreference) findPreference(preferenceKey);

            // Explicit dimensions stay fixed even when they match a preset outside the reported
            // decoder range. Custom dimensions already allow the same sizes; don't reset them here.
            if (PreferenceConfiguration.RESOLUTION_PREF_STRING.equals(preferenceKey) && value.equals(pref.getValue())) {
                return;
            }

            // Count the number of matching entries we'll be removing
            for (CharSequence seq : pref.getEntryValues()) {
                if (seq.toString().equalsIgnoreCase(value)) {
                    matchingCount++;
                }
            }

            // Create the new arrays
            CharSequence[] entries = new CharSequence[pref.getEntries().length-matchingCount];
            CharSequence[] entryValues = new CharSequence[pref.getEntryValues().length-matchingCount];
            int outIndex = 0;
            for (int i = 0; i < pref.getEntryValues().length; i++) {
                if (pref.getEntryValues()[i].toString().equalsIgnoreCase(value)) {
                    // Skip matching values
                    continue;
                }

                entries[outIndex] = pref.getEntries()[i];
                entryValues[outIndex] = pref.getEntryValues()[i];
                outIndex++;
            }

            if (pref.getValue().equalsIgnoreCase(value)) {
                onMatched.run();
            }

            // Update the preference with the new list
            pref.setEntries(entries);
            pref.setEntryValues(entryValues);
        }

        // Removes a setting this device cannot use from whichever group holds it.
        private void removeSetting(String key) {
            Preference preference = findPreference(key);
            PreferenceScreen screen = getPreferenceScreen();
            for (int i = 0; preference != null && i < screen.getPreferenceCount(); i++) {
                if (((PreferenceGroup) screen.getPreference(i)).removePreference(preference)) {
                    return;
                }
            }
        }

        private void resetBitrateToDefault(SharedPreferences prefs, String res, String fps) {
            // A bitrate the user picked stays; a default one follows the new resolution and frame rate.
            if (PreferenceConfiguration.isBitrateUserSet(prefs)) {
                return;
            }
            if (res == null) {
                res = prefs.getString(PreferenceConfiguration.RESOLUTION_PREF_STRING, PreferenceConfiguration.DEFAULT_RESOLUTION);
            }
            if (fps == null) {
                fps = prefs.getString(PreferenceConfiguration.FPS_PREF_STRING, PreferenceConfiguration.DEFAULT_FPS);
            }

            prefs.edit()
                    .putInt(PreferenceConfiguration.BITRATE_PREF_STRING,
                            PreferenceConfiguration.getDefaultBitrate(res, fps))
                    .putBoolean(PreferenceConfiguration.BITRATE_USER_SET_PREF_STRING, false)
                    .apply();
        }

        private void updateCodecSummary(ListPreference codec, String value, boolean hdr) {
            final int generation = ++codecSummaryGeneration;
            if ("forcepyrowave".equals(value)) {
                android.app.Activity activity = getActivity();
                setCodecSummary(codec, getString(R.string.pyrowave_checking));
                new Thread(() -> {
                    int summary = PyroWaveDecoderRenderer.getReadinessSummary(activity.getApplicationContext(), hdr);
                    activity.runOnUiThread(() -> {
                        if (isAdded() && getActivity() == activity && !activity.isFinishing() &&
                                generation == codecSummaryGeneration) {
                            setCodecSummary(codec, getString(summary));
                        }
                    });
                }, "GPU capability check").start();
                return;
            }
            CharSequence help = getString(R.string.summary_video_format);
            String measured = DecoderBenchmark.summary(getActivity());
            if (measured != null) {
                help = help + "\n\n" + getString(R.string.video_format_measured, measured);
            }
            setCodecSummary(codec, help);
        }

        private void setCodecSummary(ListPreference codec, CharSequence help) {
            descriptions.put("video_format", help);
            codec.setDialogMessage(help);
        }

        private void showInfo(int title, int message) {
            UiHelper.showDialog(getActivity(), new MaterialAlertDialogBuilder(getActivity()).setTitle(title)
                    .setMessage(message).setPositiveButton(android.R.string.ok, null).create());
        }

        private void showAbout() {
            AlertDialog about = new MaterialAlertDialogBuilder(getActivity()).setTitle(R.string.title_about)
                    .setMessage(getString(R.string.summary_about, BuildConfig.VERSION_NAME))
                    .setPositiveButton(android.R.string.ok, null)
                    .setNeutralButton(R.string.settings_source, (dialog, which) ->
                            HelpLauncher.launchUrl(getActivity(), "https://github.com/RamazanKara/nightjar"))
                    .create();
            UiHelper.showDialog(getActivity(), about);
        }

        private static int storedTouchMode(SharedPreferences prefs) {
            return TouchMode.resolve(prefs.getString(TouchMode.PREF, null),
                    prefs.getBoolean(TouchMode.TRACKPAD_PREF, true), prefs.getBoolean(TouchMode.NATIVE_TOUCH_PREF, false));
        }

        private static int storedVideoScaling(SharedPreferences prefs) {
            return VideoScaling.resolve(prefs.getString(VideoScaling.PREF, null),
                    prefs.getBoolean(VideoScaling.STRETCH_PREF, false));
        }

        private void confirmReset() {
            AlertDialog confirm = new MaterialAlertDialogBuilder(getActivity()).setTitle(R.string.dialog_reset_settings_title)
                    .setMessage(R.string.dialog_reset_settings_text)
                    .setNegativeButton(android.R.string.cancel, null)
                    .setPositiveButton(R.string.dialog_reset_settings_confirm, (dialog, which) -> {
                        resetAllSettings(getActivity());
                        ((StreamSettings) getActivity()).reloadSettings();
                        Toast.makeText(getActivity(), R.string.toast_reset_settings, Toast.LENGTH_SHORT).show();
                    }).create();
            UiHelper.showDialog(getActivity(), confirm);
            UiHelper.markDestructive(confirm);
        }

        @Override
        public View onCreateView(LayoutInflater inflater, ViewGroup container, Bundle savedInstanceState) {
            View view = super.onCreateView(inflater, container, savedInstanceState);
            stylePreferenceList(view, getActivity());
            return view;
        }

        @Override
        public void onCreate(Bundle savedInstanceState) {
            super.onCreate(savedInstanceState);

            addPreferencesFromResource(R.xml.preferences);
            PreferenceScreen screen = getPreferenceScreen();
            SharedPreferences settings = PreferenceManager.getDefaultSharedPreferences(getActivity());
            if (!settings.contains(PreferenceConfiguration.BITRATE_USER_SET_PREF_STRING)) {
                // Decide for older installs before a removed resolution or frame rate is replaced below.
                settings.edit().putBoolean(PreferenceConfiguration.BITRATE_USER_SET_PREF_STRING,
                        PreferenceConfiguration.isBitrateUserSet(settings)).apply();
            }
            findPreference(PreferenceConfiguration.BITRATE_PREF_STRING).setOnPreferenceChangeListener((preference, value) -> {
                settings.edit().putBoolean(PreferenceConfiguration.BITRATE_USER_SET_PREF_STRING, true).apply();
                return true;
            });
            findPreference("checkbox_codec_low_latency").setEnabled(Build.VERSION.SDK_INT >= Build.VERSION_CODES.R);
            findPreference("checkbox_codec_performance").setEnabled(Build.VERSION.SDK_INT >= Build.VERSION_CODES.M);
            findPreference("checkbox_phone_performance_hints").setEnabled(Build.VERSION.SDK_INT >= Build.VERSION_CODES.S);
            if (!PyroWaveDecoderRenderer.hasAdrenoGpu()) {
                removeSetting(PreferenceConfiguration.GPU_MAX_CLOCKS_PREF_STRING);
            }
            // The same check as the driver screen: custom drivers load on arm64 Adreno devices only.
            if (!((StreamSettings) getActivity()).supportsGpuDrivers()) {
                removeSetting("gpu_driver");
            }
            findPreference(LOW_LATENCY_DECODING_KEY).setOnPreferenceChangeListener((preference, value) -> {
                PreferenceConfiguration.setLowLatencyDecoding(getPreferenceManager().getSharedPreferences(), (Boolean) value);
                return true;
            });
            ListPreference pacing = (ListPreference) findPreference("frame_pacing");
            findPreference("checkbox_drop_late_frames").setEnabled("balanced".equals(pacing.getValue()));
            pacing.setOnPreferenceChangeListener((preference, value) -> {
                findPreference("checkbox_drop_late_frames").setEnabled("balanced".equals(value));
                return true;
            });
            ListPreference quality = (ListPreference) findPreference("adaptive_quality_mode");
            quality.setOnPreferenceChangeListener((preference, value) -> {
                // The minimums only apply while automatic quality runs.
                findPreference("adaptive_quality_min_resolution").setEnabled(!"off".equals(value));
                findPreference("adaptive_quality_min_fps").setEnabled(!"off".equals(value));
                return true;
            });
            ListPreference codec = (ListPreference) findPreference("video_format");
            SwitchPreference hdr = (SwitchPreference) findPreference("checkbox_enable_hdr");
            Preference texture = findPreference("checkbox_texture_view");
            texture.setEnabled(!hdr.isChecked() && !"forcepyrowave".equals(codec.getValue()));
            codec.setOnPreferenceChangeListener((preference, value) -> {
                texture.setEnabled(!hdr.isChecked() && !"forcepyrowave".equals(value));
                updateCodecSummary(codec, (String) value, hdr.isChecked());
                return true;
            });
            hdr.setOnPreferenceChangeListener((preference, value) -> {
                texture.setEnabled(!(Boolean) value && !"forcepyrowave".equals(codec.getValue()));
                updateCodecSummary(codec, codec.getValue(), (Boolean) value);
                return true;
            });
            findPreference("about_app").setSummary(getString(R.string.settings_version, BuildConfig.VERSION_NAME));
            findPreference("about_app").setOnPreferenceClickListener(preference -> {
                showAbout();
                return true;
            });
            findPreference("help").setOnPreferenceClickListener(preference -> {
                HelpLauncher.showHelp(getActivity());
                return true;
            });
            // Google Play, or a build made for another store, updates the app itself.
            if (UpdateFlow.isAvailable(getActivity())) {
                findPreference("check_updates_now").setOnPreferenceClickListener(preference -> {
                    UpdateFlow.checkNow(getActivity());
                    return true;
                });
            } else {
                removeSetting(UpdatePrefs.AUTO_CHECK_PREF_STRING);
                removeSetting("check_updates_now");
            }
            findPreference("host_display_location").setOnPreferenceClickListener(preference -> {
                showInfo(R.string.host_display_menu, R.string.host_display_location);
                return true;
            });
            findPreference("latency_benchmark").setOnPreferenceClickListener(preference -> {
                startActivity(new Intent(getActivity(), com.limelight.LatencyBenchmarkActivity.class));
                return true;
            });
            Preference gpuDriver = findPreference("gpu_driver");
            if (gpuDriver != null) {
                gpuDriver.setOnPreferenceClickListener(preference -> {
                    startActivity(new Intent(getActivity(), GpuDriverSettings.class));
                    return true;
                });
            }
            findPreference("report_problem").setOnPreferenceClickListener(preference -> {
                com.limelight.utils.ProblemReport.show(getActivity());
                return true;
            });
            findPreference("reset_all").setOnPreferenceClickListener(preference -> {
                confirmReset();
                return true;
            });
            findPreference("checkbox_touchscreen_trackpad").setOnPreferenceClickListener(preference -> {
                SharedPreferences prefs = getPreferenceManager().getSharedPreferences();
                int selected = storedTouchMode(prefs);
                AlertDialog picker = new MaterialAlertDialogBuilder(getActivity()).setTitle(R.string.stream_touch_mode)
                        .setSingleChoiceItems(R.array.stream_touch_modes, selected, (dialog, which) -> {
                            SharedPreferences.Editor editor = prefs.edit();
                            TouchMode.save(editor, which);
                            editor.apply();
                            dialog.dismiss();
                        }).setNeutralButton(R.string.help, null)
                        .setNegativeButton(android.R.string.cancel, null).create();
                UiHelper.showDialog(getActivity(), picker);
                // Like the other pickers, Help explains the choices without closing the list.
                picker.getButton(AlertDialog.BUTTON_NEUTRAL).setOnClickListener(view ->
                        showInfo(R.string.stream_touch_mode, R.string.settings_touch_mode_help));
                return true;
            });
            findPreference(VideoScaling.STRETCH_PREF).setOnPreferenceClickListener(preference -> {
                SharedPreferences prefs = getPreferenceManager().getSharedPreferences();
                AlertDialog picker = new MaterialAlertDialogBuilder(getActivity()).setTitle(R.string.video_scaling_title)
                        .setSingleChoiceItems(R.array.video_scaling_modes, storedVideoScaling(prefs), (dialog, which) -> {
                            SharedPreferences.Editor editor = prefs.edit();
                            VideoScaling.save(editor, which);
                            editor.apply();
                            dialog.dismiss();
                        }).setNeutralButton(R.string.help, null)
                        .setNegativeButton(android.R.string.cancel, null).create();
                UiHelper.showDialog(getActivity(), picker);
                picker.getButton(AlertDialog.BUTTON_NEUTRAL).setOnClickListener(view ->
                        showInfo(R.string.video_scaling_title, R.string.video_scaling_help));
                return true;
            });
            findPreference("controller_button_mapping").setOnPreferenceClickListener(preference -> {
                startActivity(new Intent(getActivity(), ControllerMappingActivity.class));
                return true;
            });
            Preference profiles = findPreference("stream_profiles");
            if (profiles != null) {
                profiles.setOnPreferenceClickListener(preference -> {
                    startActivity(new Intent(getActivity(), StreamProfilesActivity.class));
                    return true;
                });
            }
            findPreference("controller_test").setOnPreferenceClickListener(preference -> {
                startActivity(new Intent(getActivity(), ControllerTestActivity.class));
                return true;
            });
            findPreference("keyboard_shortcuts_editor").setOnPreferenceClickListener(preference -> {
                startActivity(new Intent(getActivity(), KeyboardShortcutsActivity.class));
                return true;
            });
            findPreference("export_latency_csv").setOnPreferenceClickListener(new Preference.OnPreferenceClickListener() {
                @Override
                public boolean onPreferenceClick(Preference preference) {
                    ((StreamSettings) getActivity()).exportLatencyCsv();
                    return true;
                }
            });

            // A TV does not turn.
            if (getActivity().getPackageManager().hasSystemFeature(PackageManager.FEATURE_LEANBACK)) {
                removeSetting(StreamOrientation.PREF);
            }

            // hide on-screen controls category on non touch screen devices
            if (!getActivity().getPackageManager().hasSystemFeature(PackageManager.FEATURE_TOUCHSCREEN)) {
                PreferenceCategory category =
                        (PreferenceCategory) findPreference("category_onscreen_controls");
                screen.removePreference(category);
            }

            // Hide remote desktop mouse mode on pre-Oreo (which doesn't have pointer capture)
            // and NVIDIA SHIELD devices (which support raw mouse input in pointer capture mode)
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O ||
                    getActivity().getPackageManager().hasSystemFeature("com.nvidia.feature.shield")) {
                removeSetting("checkbox_absolute_mouse_mode");
            }

            // Hide gamepad motion sensor option when running on OSes before Android 12.
            // Support for motion, LED, battery, and other extensions were introduced in S.
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) {
                removeSetting("checkbox_gamepad_motion_sensors");
            }

            // Hide gamepad motion sensor fallback option if the device has no gyro or accelerometer
            if (!getActivity().getPackageManager().hasSystemFeature(PackageManager.FEATURE_SENSOR_ACCELEROMETER) &&
                    !getActivity().getPackageManager().hasSystemFeature(PackageManager.FEATURE_SENSOR_GYROSCOPE)) {
                removeSetting("checkbox_gamepad_motion_fallback");
            }

            // The microphone switch turns on only once microphone access is allowed.
            SwitchPreference mic = (SwitchPreference) findPreference(PreferenceConfiguration.ENABLE_MIC_PREF_STRING);
            mic.setOnPreferenceChangeListener((preference, value) -> {
                if (!Boolean.TRUE.equals(value) || hasMicPermission()) {
                    return true;
                }
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                    requestPermissions(new String[] {Manifest.permission.RECORD_AUDIO}, MIC_PERMISSION_REQUEST);
                }
                return false;
            });

            SwitchPreference dualSense = (SwitchPreference) findPreference("checkbox_usb_dualsense");
            dualSense.setOnPreferenceChangeListener((preference, value) -> {
                if (!Boolean.TRUE.equals(value)) {
                    return true;
                }
                new MaterialAlertDialogBuilder(getActivity())
                        .setTitle(R.string.title_usb_dualsense)
                        .setMessage(R.string.usb_dualsense_permission_info)
                        .setPositiveButton(R.string.usb_dualsense_enable, (dialog, which) -> dualSense.setChecked(true))
                        .setNegativeButton(android.R.string.cancel, null)
                        .show();
                return false;
            });

            // Hide USB driver options on devices without USB host support
            if (!((StreamSettings) getActivity()).supportsUsbHost()) {
                screen.removePreference(findPreference("category_usb_controllers"));
            }

            // Remove PiP mode on devices pre-Oreo, where the feature is not available (some low RAM devices),
            // and on Fire OS where it violates the Amazon App Store guidelines for some reason.
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O ||
                    !getActivity().getPackageManager().hasSystemFeature("android.software.picture_in_picture") ||
                    getActivity().getPackageManager().hasSystemFeature("com.amazon.software.fireos")) {
                removeSetting("checkbox_enable_pip");
            }

            // Remove the vibration options if the device can't vibrate
            if (!((Vibrator)getActivity().getSystemService(Context.VIBRATOR_SERVICE)).hasVibrator()) {
                removeSetting("checkbox_vibrate_fallback");
                removeSetting("seekbar_vibrate_fallback_strength");
                // The entire OSC category may have already been removed by the touchscreen check above
                removeSetting("checkbox_vibrate_osc");
            }
            else if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O ||
                    !((Vibrator)getActivity().getSystemService(Context.VIBRATOR_SERVICE)).hasAmplitudeControl() ) {
                // Remove the vibration strength selector of the device doesn't have amplitude control
                removeSetting("seekbar_vibrate_fallback_strength");
            }

            Display display = getActivity().getWindowManager().getDefaultDisplay();
            float maxSupportedFps = display.getRefreshRate();
            boolean television = getActivity().getPackageManager().hasSystemFeature(PackageManager.FEATURE_TELEVISION);

            // Hide non-supported resolution/FPS combinations
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                int maxSupportedResW = 0;

                // Add a native resolution with any insets included for users that don't want content
                // behind the notch of their display
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                    DisplayCutout cutout;

                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                        // Use the much nicer Display.getCutout() API on Android 10+
                        cutout = display.getCutout();
                    }
                    else {
                        // Android 9 only
                        WindowInsets insets = getActivity().getWindow().getDecorView().getRootWindowInsets();
                        cutout = insets == null ? null : insets.getDisplayCutout();
                    }

                    if (cutout != null) {
                        int widthInsets = cutout.getSafeInsetLeft() + cutout.getSafeInsetRight();
                        int heightInsets = cutout.getSafeInsetBottom() + cutout.getSafeInsetTop();

                        if (widthInsets != 0 || heightInsets != 0) {
                            DisplayMetrics metrics = new DisplayMetrics();
                            display.getRealMetrics(metrics);

                            int width = Math.max(metrics.widthPixels - widthInsets, metrics.heightPixels - heightInsets);
                            int height = Math.min(metrics.widthPixels - widthInsets, metrics.heightPixels - heightInsets);

                            addNativeResolutionEntries(width, height, true);
                        }
                    }
                }

                // Always allow resolutions that are smaller or equal to the active
                // display resolution because decoders can report total non-sense to us.
                // For example, a p201 device reports:
                // AVC Decoder: OMX.amlogic.avc.decoder.awesome
                // HEVC Decoder: OMX.amlogic.hevc.decoder.awesome
                // AVC supported width range: 64 - 384
                // HEVC supported width range: 64 - 544
                for (Display.Mode candidate : display.getSupportedModes()) {
                    // Some devices report their dimensions in the portrait orientation
                    // where height > width. Normalize these to the conventional width > height
                    // arrangement before we process them.

                    int width = Math.max(candidate.getPhysicalWidth(), candidate.getPhysicalHeight());
                    int height = Math.min(candidate.getPhysicalWidth(), candidate.getPhysicalHeight());

                    // Some TVs report strange values here, so let's avoid native resolutions on a TV
                    // unless they report greater than 4K resolutions.
                    if (!television || (width > 3840 || height > 2160)) {
                        addNativeResolutionEntries(width, height, false);
                    }

                    if ((width >= 3840 || height >= 2160) && maxSupportedResW < 3840) {
                        maxSupportedResW = 3840;
                    }
                    else if ((width >= 2560 || height >= 1440) && maxSupportedResW < 2560) {
                        maxSupportedResW = 2560;
                    }
                    else if ((width >= 1920 || height >= 1080) && maxSupportedResW < 1920) {
                        maxSupportedResW = 1920;
                    }

                    if (candidate.getRefreshRate() > maxSupportedFps) {
                        maxSupportedFps = candidate.getRefreshRate();
                    }
                }

                // This must be called to do runtime initialization before calling functions that evaluate
                // decoder lists.
                MediaCodecHelper.initialize(getContext(), GlPreferences.readPreferences(getContext()).glRenderer);

                MediaCodecInfo avcDecoder = MediaCodecHelper.findProbableSafeDecoder("video/avc", -1);
                MediaCodecInfo hevcDecoder = MediaCodecHelper.findProbableSafeDecoder("video/hevc", -1);

                if (avcDecoder != null) {
                    Range<Integer> avcWidthRange = avcDecoder.getCapabilitiesForType("video/avc").getVideoCapabilities().getSupportedWidths();

                    LimeLog.info("AVC supported width range: "+avcWidthRange.getLower()+" - "+avcWidthRange.getUpper());

                    // If 720p is not reported as supported, ignore all results from this API
                    if (avcWidthRange.contains(1280)) {
                        if (avcWidthRange.contains(3840) && maxSupportedResW < 3840) {
                            maxSupportedResW = 3840;
                        }
                        else if (avcWidthRange.contains(1920) && maxSupportedResW < 1920) {
                            maxSupportedResW = 1920;
                        }
                        else if (maxSupportedResW < 1280) {
                            maxSupportedResW = 1280;
                        }
                    }
                }

                if (hevcDecoder != null) {
                    Range<Integer> hevcWidthRange = hevcDecoder.getCapabilitiesForType("video/hevc").getVideoCapabilities().getSupportedWidths();

                    LimeLog.info("HEVC supported width range: "+hevcWidthRange.getLower()+" - "+hevcWidthRange.getUpper());

                    // If 720p is not reported as supported, ignore all results from this API
                    if (hevcWidthRange.contains(1280)) {
                        if (hevcWidthRange.contains(3840) && maxSupportedResW < 3840) {
                            maxSupportedResW = 3840;
                        }
                        else if (hevcWidthRange.contains(1920) && maxSupportedResW < 1920) {
                            maxSupportedResW = 1920;
                        }
                        else if (maxSupportedResW < 1280) {
                            maxSupportedResW = 1280;
                        }
                    }
                }

                LimeLog.info("Maximum resolution slot: "+maxSupportedResW);

                if (maxSupportedResW != 0) {
                    if (maxSupportedResW < 3840) {
                        // 4K is unsupported
                        removeValue(PreferenceConfiguration.RESOLUTION_PREF_STRING, PreferenceConfiguration.RES_4K, new Runnable() {
                            @Override
                            public void run() {
                                SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(SettingsFragment.this.getActivity());
                                setValue(PreferenceConfiguration.RESOLUTION_PREF_STRING, PreferenceConfiguration.RES_1440P);
                                resetBitrateToDefault(prefs, null, null);
                            }
                        });
                    }
                    if (maxSupportedResW < 2560) {
                        // 1440p is unsupported
                        removeValue(PreferenceConfiguration.RESOLUTION_PREF_STRING, PreferenceConfiguration.RES_1440P, new Runnable() {
                            @Override
                            public void run() {
                                SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(SettingsFragment.this.getActivity());
                                setValue(PreferenceConfiguration.RESOLUTION_PREF_STRING, PreferenceConfiguration.RES_1080P);
                                resetBitrateToDefault(prefs, null, null);
                            }
                        });
                    }
                    if (maxSupportedResW < 1920) {
                        // 1080p is unsupported
                        removeValue(PreferenceConfiguration.RESOLUTION_PREF_STRING, PreferenceConfiguration.RES_1080P, new Runnable() {
                            @Override
                            public void run() {
                                SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(SettingsFragment.this.getActivity());
                                setValue(PreferenceConfiguration.RESOLUTION_PREF_STRING, PreferenceConfiguration.RES_720P);
                                resetBitrateToDefault(prefs, null, null);
                            }
                        });
                    }
                    // Never remove 720p
                }
            }
            else {
                // We can get the true metrics via the getRealMetrics() function (unlike the lies
                // that getWidth() and getHeight() tell to us).
                DisplayMetrics metrics = new DisplayMetrics();
                display.getRealMetrics(metrics);
                int width = Math.max(metrics.widthPixels, metrics.heightPixels);
                int height = Math.min(metrics.widthPixels, metrics.heightPixels);
                addNativeResolutionEntries(width, height, false);
            }
            // TVs can report a smaller panel than the screen they drive, so they keep every size.
            ((ResolutionPreference) findPreference(PreferenceConfiguration.RESOLUTION_PREF_STRING))
                    .ensureSelectedResolutionEntry();
            sortResolutionEntries(!television);

            if (!PreferenceConfiguration.readPreferences(this.getActivity()).unlockFps) {
                // We give some extra room in case the FPS is rounded down
                if (maxSupportedFps < 118) {
                    removeValue(PreferenceConfiguration.FPS_PREF_STRING, "120", new Runnable() {
                        @Override
                        public void run() {
                            SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(SettingsFragment.this.getActivity());
                            setValue(PreferenceConfiguration.FPS_PREF_STRING, "90");
                            resetBitrateToDefault(prefs, null, null);
                        }
                    });
                }
                if (maxSupportedFps < 88) {
                    // 1080p is unsupported
                    removeValue(PreferenceConfiguration.FPS_PREF_STRING, "90", new Runnable() {
                        @Override
                        public void run() {
                            SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(SettingsFragment.this.getActivity());
                            setValue(PreferenceConfiguration.FPS_PREF_STRING, "60");
                            resetBitrateToDefault(prefs, null, null);
                        }
                    });
                }
                // Never remove 30 FPS or 60 FPS
            }
            addNativeFrameRateEntry(maxSupportedFps);

            // Android L introduces the drop duplicate behavior of releaseOutputBuffer()
            // that the unlock FPS option relies on to not massively increase latency.
            findPreference(PreferenceConfiguration.UNLOCK_FPS_STRING).setOnPreferenceChangeListener(new Preference.OnPreferenceChangeListener() {
                @Override
                public boolean onPreferenceChange(Preference preference, Object newValue) {
                    // HACK: We need to let the preference change succeed before reinitializing to ensure
                    // it's reflected in the new layout.
                    final Handler h = new Handler();
                    h.postDelayed(new Runnable() {
                        @Override
                        public void run() {
                            // Ensure the activity is still open when this timeout expires
                            StreamSettings settingsActivity = (StreamSettings) SettingsFragment.this.getActivity();
                            if (settingsActivity != null) {
                                settingsActivity.reloadSettings();
                            }
                        }
                    }, 500);

                    // Allow the original preference change to take place
                    return true;
                }
            });

            // Remove HDR preference for devices below Nougat
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.N) {
                LimeLog.info("Excluding HDR toggle based on OS");
                removeSetting("checkbox_enable_hdr");
            }
            else {
                Display.HdrCapabilities hdrCaps = display.getHdrCapabilities();

                // We must now ensure our display is compatible with HDR10
                boolean foundHdr10 = false;
                if (hdrCaps != null) {
                    // getHdrCapabilities() returns null on Lenovo Lenovo Mirage Solo (vega), Android 8.0
                    for (int hdrType : hdrCaps.getSupportedHdrTypes()) {
                        if (hdrType == Display.HdrCapabilities.HDR_TYPE_HDR10) {
                            foundHdr10 = true;
                            break;
                        }
                    }
                }

                if (!foundHdr10) {
                    LimeLog.info("Excluding HDR toggle based on display capabilities");
                    removeSetting("checkbox_enable_hdr");
                }
                else if (PreferenceConfiguration.isShieldAtvFirmwareWithBrokenHdr()) {
                    LimeLog.info("Disabling HDR toggle on old broken SHIELD TV firmware");
                    hdr.setEnabled(false);
                    hdr.setChecked(false);
                    hdr.setSummary("Update the firmware on your NVIDIA SHIELD Android TV to enable HDR");
                }
            }

            // Add a listener to the FPS and resolution preference
            // so the bitrate can be auto-adjusted
            findPreference(PreferenceConfiguration.RESOLUTION_PREF_STRING).setOnPreferenceChangeListener(new Preference.OnPreferenceChangeListener() {
                @Override
                public boolean onPreferenceChange(Preference preference, Object newValue) {
                    SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(SettingsFragment.this.getActivity());
                    String valueStr = (String) newValue;

                    // Picking this screen's native size keeps following the screen; any other size stays fixed.
                    prefs.edit().putBoolean(PreferenceConfiguration.RESOLUTION_FOLLOWS_SCREEN_PREF_STRING,
                            !((ResolutionPreference) preference).isChoosingCustomResolution() &&
                                    valueStr.equals(PreferenceConfiguration.nativeResolution(getActivity()))).apply();

                    // Write the new bitrate value
                    resetBitrateToDefault(prefs, valueStr, null);

                    // Allow the original preference change to take place
                    return true;
                }
            });
            findPreference(PreferenceConfiguration.FPS_PREF_STRING).setOnPreferenceChangeListener(new Preference.OnPreferenceChangeListener() {
                @Override
                public boolean onPreferenceChange(Preference preference, Object newValue) {
                    SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(SettingsFragment.this.getActivity());
                    String valueStr = (String) newValue;

                    // Write the new bitrate value
                    resetBitrateToDefault(prefs, null, valueStr);

                    // Allow the original preference change to take place
                    return true;
                }
            });

            for (int i = 0; i < screen.getPreferenceCount(); i++) {
                PreferenceGroup group = (PreferenceGroup) screen.getPreference(i);
                for (int j = 0; j < group.getPreferenceCount(); j++) {
                    Preference pref = group.getPreference(j);
                    descriptions.put(pref.getKey(), pref instanceof MaterialSwitchPreference ?
                            ((MaterialSwitchPreference) pref).getExplanation() :
                            pref instanceof android.preference.DialogPreference ?
                                    ((android.preference.DialogPreference) pref).getDialogMessage() : pref.getSummary());
                }
            }
            updateCodecSummary(codec, codec.getValue(), hdr.isChecked());
            stylePreferences(screen);
            updateValueSummaries(screen);
            for (int i = 0; i < screen.getPreferenceCount(); i++) {
                PreferenceCategory category = (PreferenceCategory) screen.getPreference(i);
                List<Preference> preferences = new ArrayList<>();
                allPreferences.put(category.getKey(), category);
                for (int j = 0; j < category.getPreferenceCount(); j++) {
                    Preference pref = category.getPreference(j);
                    preferences.add(pref);
                    allPreferences.put(pref.getKey(), pref);
                }
                // A group whose settings this device cannot use is left out entirely.
                if (!preferences.isEmpty()) {
                    searchCategories.put(category, preferences);
                }
            }
            screen.removeAll();
            filterPreferences();
        }
    }
}

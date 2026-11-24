package com.limelight.preferences;

import android.Manifest;
import android.content.Context;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.os.Build;
import android.os.Bundle;
import android.preference.ListPreference;
import android.preference.Preference;
import android.preference.PreferenceCategory;
import android.preference.PreferenceFragment;
import android.preference.PreferenceGroup;
import android.preference.PreferenceManager;
import android.preference.PreferenceScreen;
import android.preference.SwitchPreference;
import android.view.LayoutInflater;
import android.view.View;
import android.view.ViewGroup;
import android.widget.ListView;

import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.binding.video.DisplayFrameRatePolicy;
import com.limelight.ui.NavigationDialogBuilder;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * Base preference fragment for {@link StreamSettings} sections and search results, coordinating
 * visibility rules, folding, live summaries, and fragment lifecycle. Subclassed by
 * {@link StreamSettings.SettingsFragment} to preserve the nested fragment class name.
 */
public abstract class StreamSettingsFragment extends PreferenceFragment {
    static final String ARG_SECTION = "section";
    static final int MIC_PERMISSION_REQUEST = 2;

    final Map<PreferenceCategory, List<Preference>> searchCategories = new LinkedHashMap<>();
    final Map<String, Preference> allPreferences = new HashMap<>();
    /** The category each setting belongs to, so a setting taken out of it can go back to its place. */
    final Map<String, PreferenceGroup> homes = new HashMap<>();
    /** Settings that do not apply right now: out of the list and out of search. */
    final Set<String> hidden = new HashSet<>();
    /** Settings behind Customize or their section's More row while it is closed: out of the list, still found by search. */
    final Set<String> folded = new HashSet<>();
    /**
     * Rows of SettingRules.WHEN_ROWS that can do nothing now and hold their default: out of the list and its More
     * count, found by search with the reason.
     */
    final Set<String> idle = new HashSet<>();
    /** Each section's main rows (settings_tiers.xml), read once. */
    private final Map<String, Set<String>> mainKeys = new HashMap<>();
    // Explanations that search matches besides the visible title and value
    final Map<String, CharSequence> descriptions = new HashMap<>();
    int displayedResultCount;
    int codecSummaryGeneration;
    Thread decoderProbe;
    private com.limelight.binding.audio.MicProcessing.Availability micEffects;

    /** Attaching or removing a mouse, keyboard or touchpad shows or hides their rows at once. */
    private final android.hardware.input.InputManager.InputDeviceListener deviceListener =
            new android.hardware.input.InputManager.InputDeviceListener() {
                @Override public void onInputDeviceAdded(int deviceId) { devicesChanged(); }
                @Override public void onInputDeviceRemoved(int deviceId) { devicesChanged(); }
                @Override public void onInputDeviceChanged(int deviceId) { devicesChanged(); }
            };

    private final SharedPreferences.OnSharedPreferenceChangeListener summaryUpdater =
            (prefs, key) -> {
                // Only the VRR keys change what the capability scan reads; a simple choice
                // clears the cache itself (SimpleProfile.apply).
                if (PreferenceConfiguration.VRR_PREF_STRING.equals(key)
                        || PreferenceConfiguration.REAL_VRR_PREF_STRING.equals(key)
                        || SimpleProfile.VRR_OPT_IN.equals(key) || SimpleProfile.VRR_LAST.equals(key)) {
                    SimpleProfile.forgetCapabilities();
                }
                StreamSettings screen = (StreamSettings) getActivity();
                // A row set back to its default while it does nothing stays for this visit, so the edit
                // does not lose its row and focus.
                if (screen != null && SettingRules.WHEN_ROWS.contains(key) && !idle.contains(key) &&
                        !SettingRules.whenApplies(screen, key, prefs, screen.devices()) &&
                        !SettingRules.changed(screen, key, prefs)) {
                    screen.revealed.add(key);
                }
                refreshState();
                // Show debug settings, or a changed Debug setting, adds or removes Debug's chip.
                if (screen != null) screen.ui.updateChips();
                if (PreferenceConfiguration.USB_FORWARDING_PREF_STRING.equals(key)) {
                    com.limelight.binding.usb.UsbForwarding.syncPersistentPermission(getActivity());
                }
            };

    void devicesChanged() {
        StreamSettings activity = (StreamSettings) getActivity();
        if (!isAdded() || activity == null || activity.isFinishing()) return;
        refreshState();
        if (!activity.searchQuery.isEmpty()) filterPreferences();
    }

    @Override
    public Preference findPreference(CharSequence key) {
        Preference preference = super.findPreference(key);
        return preference != null ? preference : allPreferences.get(key.toString());
    }

    void filterPreferences() {
        StreamSettingsSearchAndSummaries.filterPreferences(this);
    }

    /** Values and summaries follow the stored settings; so do the settings that apply only given the others. */
    void refreshState() {
        // Rules first: a setting that comes back shows the stored value.
        applySettingRules(getPreferenceManager().getSharedPreferences());
        StreamSettingsSearchAndSummaries.updateValueSummaries(this, getPreferenceScreen());
        StreamSettingsSearchAndSummaries.showWhenReasons(this, getPreferenceManager().getSharedPreferences());
    }

    /** Records whether a setting applies; placeRows then puts it in or out of the list. */
    void setShown(String key, boolean shown) {
        if (shown) hidden.remove(key);
        else hidden.add(key);
    }

    private void place(String key, boolean attached) {
        Preference pref = allPreferences.get(key);
        PreferenceGroup home = homes.get(key);
        if (pref == null || home == null) return;
        if (attached) home.addPreference(pref);
        else home.removePreference(pref);
    }

    /**
     * A setting that does nothing for the current choices is hidden, not greyed, and keeps its value: frame
     * pacing while adaptive refresh replaces it (its On shows frames at the PC's frame times), the sharpness without an upscaler that sharpens, the two minimums while automatic quality is
     * off, a switch's own rows while it is off (SettingRules), and the new streaming engine's rows with the
     * classic one. Hidden settings leave search and their section's More count, and come back in their place.
     */
    private void applySettingRules(SharedPreferences prefs) {
        Context context = getActivity();
        setShown(PreferenceConfiguration.BITRATE_PREF_STRING, SettingRules.bitrateShown(prefs));
        setShown(StreamSettings.SHARP_COLOR_KEY, SettingRules.sharpColorApplies(prefs));
        setShown("checkbox_enable_sdr_10bit", SettingRules.sdr10Applies(prefs));
        setShown(SettingRules.FRAME_PACING, SettingRules.framePacingApplies(context, prefs));
        setShown("checkbox_reduce_refresh_rate", SettingRules.reduceRefreshRateApplies(context, prefs));
        setShown(PreferenceConfiguration.ARR_FIXED_TIMING_PREF_STRING, SettingRules.arrFixedTimingShown(context, prefs));
        setShown("checkbox_display_queue_recovery", SettingRules.displayQueueRecoveryApplies(context, prefs));
        setShown(PreferenceConfiguration.UPSCALING_PREF_STRING, SettingRules.upscalingApplies(prefs));
        setShown("video_alignment", SettingRules.videoAlignmentApplies(prefs));
        setShown("seekbar_virtual_display_scale", SettingRules.virtualDisplayScaleApplies(prefs));
        boolean controllers = SettingRules.controllersOn(prefs);
        for (String key : SettingRules.CONTROLLER_ROWS) setShown(key, controllers);
        boolean controllerMouse = SettingRules.controllerMouseApplies(prefs);
        setShown("analog_scrolling", controllerMouse);
        setShown("checkbox_mouse_emulation_fixed_step", controllerMouse);
        setShown("checkbox_usb_bind_all", prefs.getBoolean("checkbox_usb_driver", true));
        setShown("checkbox_usb_dualsense_haptics", prefs.getBoolean("checkbox_usb_dualsense", false));
        boolean onScreen = SettingRules.onScreenControlsOn(prefs);
        for (String key : SettingRules.ON_SCREEN_ROWS) setShown(key, onScreen);
        boolean mic = SettingRules.micOn(prefs);
        for (String key : SettingRules.MIC_ROWS) setShown(key, mic);
        // The rows that do nothing in this mode or on this device leave the list but not search, and a changed value
        // keeps its row (SettingRules.WHEN_ROWS).
        StreamSettings activity = (StreamSettings) getActivity();
        idle.clear();
        for (String key : SettingRules.WHEN_ROWS) {
            if (!activity.revealed.contains(key) && !SettingRules.whenApplies(context, key, prefs, activity.devices()) &&
                    !SettingRules.changed(context, key, prefs)) {
                idle.add(key);
            }
        }
        setShown(PreferenceConfiguration.AUDIO_PASSTHROUGH_PREF_STRING,
                SettingRules.audioPassthroughApplies(Build.VERSION.SDK_INT));
        placeRows();
    }

    /**
     * Puts every row in or out of its group: out while it does not apply, or while Customize or its section's More
     * row is closed. Rows come back in XML order, so a setting is attached before the ones that depend on it. Then
     * the More row counts what it holds, and only groups with a row show.
     */
    void placeRows() {
        StreamSettings activity = (StreamSettings) getActivity();
        if (activity == null) return;
        folded.clear();
        Map<String, Integer> foldedCount = new HashMap<>();
        for (Map.Entry<PreferenceCategory, List<Preference>> entry : searchCategories.entrySet()) {
            String rowSection = StreamSettings.sectionOf(entry.getKey().getKey());
            if (rowSection != null && !mainKeys.containsKey(rowSection)) {
                mainKeys.put(rowSection, new HashSet<>(StreamSettings.mainKeys(activity, rowSection)));
            }
            for (Preference pref : entry.getValue()) {
                String key = pref.getKey();
                boolean behindMore = rowSection != null && !key.equals(StreamSettings.MORE_PREFIX + rowSection) &&
                        !StreamSettings.LOW_LATENCY_CUSTOMIZED.contains(key) && !mainKeys.get(rowSection).contains(key);
                if (behindMore && !hidden.contains(key) && !idle.contains(key)) foldedCount.merge(rowSection, 1, Integer::sum);
                if (StreamSettings.LOW_LATENCY_CUSTOMIZED.contains(key) ? !activity.customizeOpen :
                        behindMore && !activity.moreOpen.contains(rowSection)) {
                    folded.add(key);
                }
            }
        }
        for (String section : StreamSettings.SECTIONS.keySet()) {
            String key = StreamSettings.MORE_PREFIX + section;
            Preference more = allPreferences.get(key);
            Integer count = foldedCount.get(section);
            // A section whose other rows this device cannot use, or that do not apply now, has nothing to unfold.
            if (count == null) {
                hidden.add(key);
            } else {
                hidden.remove(key);
                if (more != null) {
                    String title = getString(StreamSettings.sectionTitle(section));
                    more.setTitle(activity.moreOpen.contains(section) ? getString(R.string.settings_fewer, title) :
                            getString(R.string.settings_more_count, title, count));
                }
            }
        }
        for (List<Preference> rows : searchCategories.values()) {
            for (Preference pref : rows) {
                String key = pref.getKey();
                place(key, !hidden.contains(key) && !idle.contains(key) && !folded.contains(key));
            }
        }
        showGroups();
    }

    /** The section's groups that have a row to show are in the list, in their place; the others are out of it. */
    private void showGroups() {
        String section = StreamSettings.normalizeSection(getArguments() == null ? null : getArguments().getString(ARG_SECTION));
        PreferenceScreen screen = getPreferenceScreen();
        if (section == null || screen == null || !((StreamSettings) getActivity()).searchQuery.isEmpty()) return;
        for (String key : StreamSettings.SECTIONS.get(section)) {
            Preference group = allPreferences.get(key);
            if (!(group instanceof PreferenceGroup)) continue;
            if (((PreferenceGroup) group).getPreferenceCount() > 0) screen.addPreference(group);
            else screen.removePreference(group);
        }
    }

    /** What this device offers, asked once: Android answers the same until it updates. */
    com.limelight.binding.audio.MicProcessing.Availability micEffects() {
        if (micEffects == null) {
            java.util.EnumMap<com.limelight.binding.audio.MicProcessing.Effect, Boolean> has =
                    new java.util.EnumMap<>(com.limelight.binding.audio.MicProcessing.Effect.class);
            for (com.limelight.binding.audio.MicProcessing.Effect effect : com.limelight.binding.audio.MicProcessing.Effect.values()) {
                has.put(effect, com.limelight.binding.audio.MicEffects.DEVICE.has(effect));
            }
            boolean own = com.limelight.binding.audio.EchoCanceller.isAvailable();
            micEffects = new com.limelight.binding.audio.MicProcessing.Availability() {
                @Override public boolean has(com.limelight.binding.audio.MicProcessing.Effect effect) {
                    return Boolean.TRUE.equals(has.get(effect));
                }

                @Override public boolean hasOwnEchoCanceller() {
                    return own;
                }
            };
        }
        return micEffects;
    }

    boolean hasMicPermission() {
        return getActivity().checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED;
    }

    private void showMicPermissionDenied() {
        new NavigationDialogBuilder(getActivity())
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

    /** The Screen row: the size and rate the simple layer streams at, read when the page opens. */
    void updateSimpleScreenSummary() {
        Preference row = findPreference("simple_screen");
        String nativeResolution = PreferenceConfiguration.nativeResolution(getActivity());
        if (row == null || nativeResolution == null) return;
        int refresh = DisplayFrameRatePolicy.streamFrameRate(DisplayFrameRatePolicy.maxRefreshRate(
                PreferenceConfiguration.defaultDisplay(getActivity())));
        row.setSummary(getString(R.string.simple_screen_value,
                PreferenceConfiguration.getWidthFromResolutionString(nativeResolution),
                PreferenceConfiguration.getHeightFromResolutionString(nativeResolution), refresh));
    }

    @Override
    public void onResume() {
        super.onResume();
        SimpleProfile.forgetCapabilities();
        updateSimpleScreenSummary();
        // Microphone access can be withdrawn in Android settings; the switch follows it.
        SwitchPreference mic = (SwitchPreference) findPreference(PreferenceConfiguration.ENABLE_MIC_PREF_STRING);
        if (mic != null && mic.isChecked() && !hasMicPermission()) {
            mic.setChecked(false);
            showMicPermissionDenied();
        }
        getPreferenceManager().getSharedPreferences().registerOnSharedPreferenceChangeListener(summaryUpdater);
        android.hardware.input.InputManager input = (android.hardware.input.InputManager)
                getActivity().getSystemService(Context.INPUT_SERVICE);
        if (input != null) input.registerInputDeviceListener(deviceListener, null);
        ListPreference codec = (ListPreference) findPreference("video_format");
        if (codec != null) {
            StreamSettingsSearchAndSummaries.updateCodecSummary(this, codec, codec.getValue(),
                    getPreferenceManager().getSharedPreferences().getBoolean("checkbox_enable_hdr", false));
        }
        refreshState();
        StreamSettingsRowBindings.refreshNetworkTests(this);
        // The fragment is replaced after the window got its insets, so ask for them again
        // to keep the last settings clear of the navigation bar.
        if (getView() != null) {
            getView().requestApplyInsets();
            StreamSettings activity = (StreamSettings) getActivity();
            if (activity.searchResultKey != null) {
                String key = activity.searchResultKey;
                boolean focus = activity.focusRow;
                activity.searchResultKey = null;
                activity.focusRow = false;
                ListView list = getView().findViewById(android.R.id.list);
                list.post(() -> {
                    int position = StreamSettings.RootFragment.positionOf(list, key);
                    if (position >= 0) {
                        // Below the top fade, as focusContentPane places a row: at the very top the fade covered its ring.
                        list.setSelectionFromTop(position, list.getVerticalFadingEdgeLength());
                        if (activity.twoPane && focus) list.requestFocus();
                    } else if (activity.twoPane) {
                        // A row of the list of sections, such as the presets.
                        ListView sections = activity.sectionList();
                        int row = sections == null ? -1 : StreamSettings.RootFragment.positionOf(sections, key);
                        if (row >= 0) {
                            sections.setSelection(row);
                            if (focus) sections.requestFocus();
                        }
                    }
                });
            }
        }
    }

    @Override
    public void onPause() {
        getPreferenceManager().getSharedPreferences().unregisterOnSharedPreferenceChangeListener(summaryUpdater);
        android.hardware.input.InputManager input = (android.hardware.input.InputManager)
                getActivity().getSystemService(Context.INPUT_SERVICE);
        if (input != null) input.unregisterInputDeviceListener(deviceListener);
        super.onPause();
    }

    // Removes a setting this device cannot use from whichever group holds it.
    void removeSetting(String key) {
        Preference preference = findPreference(key);
        PreferenceScreen screen = getPreferenceScreen();
        for (int i = 0; preference != null && i < screen.getPreferenceCount(); i++) {
            if (((PreferenceGroup) screen.getPreference(i)).removePreference(preference)) {
                return;
            }
        }
    }

    boolean customizeOpen() {
        return getActivity() != null && ((StreamSettings) getActivity()).customizeOpen;
    }

    /**
     * Opens or closes a section's More row in place. With a D-pad the selection goes to the first row it showed,
     * or back to the More row when it closes, instead of staying on whatever row now sits there.
     */
    void toggleMore(String section) {
        StreamSettings activity = (StreamSettings) getActivity();
        boolean open = !activity.moreOpen.remove(section);
        if (open) activity.moreOpen.add(section);
        activity.rememberMore(section, open);
        refreshState();
        String first = open ? firstUnfolded(section) : null;
        String select = first != null ? first : StreamSettings.MORE_PREFIX + section;
        ListView list = getView() == null ? null : getView().findViewById(android.R.id.list);
        if (list == null || list.isInTouchMode()) return;
        list.post(() -> {
            int position = StreamSettings.RootFragment.positionOf(list, select);
            // Below the top fade, as focusContentPane places a row: at the very top the fade covered its ring.
            if (position >= 0) list.setSelectionFromTop(position, list.getVerticalFadingEdgeLength());
        });
    }

    /** The first row, in list order, that the section's More row holds and that applies now; null when none. */
    String firstUnfolded(String section) {
        for (Map.Entry<PreferenceCategory, List<Preference>> entry : searchCategories.entrySet()) {
            if (!section.equals(StreamSettings.sectionOf(entry.getKey().getKey()))) continue;
            for (Preference pref : entry.getValue()) {
                String key = pref.getKey();
                if (!hidden.contains(key) && !idle.contains(key) && StreamSettings.foldsInSection(getActivity(), section, key)) {
                    return key;
                }
            }
        }
        return null;
    }

    @Override
    public View onCreateView(LayoutInflater inflater, ViewGroup container, Bundle savedInstanceState) {
        View view = super.onCreateView(inflater, container, savedInstanceState);
        StreamSettingsChrome.stylePreferenceList(view, getActivity(), true);
        return view;
    }

    @Override
    public void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);

        SharedPreferences settings = PreferenceManager.getDefaultSharedPreferences(getActivity());
        // Before the lists read their values, so an older Close game choice shows as Quit game.
        com.limelight.ui.StreamMenu.upgradeButtonChoices(settings);
        addPreferencesFromResource(R.xml.preferences);
        PreferenceScreen screen = getPreferenceScreen();
        SwitchPreference hdr = StreamSettingsRowBindings.bindRowsAndPruneHardware(this, screen, settings);
        StreamSettingsDisplayOptions.configureDisplayAndMediaCapabilities(this, hdr);

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
        ListPreference codec = (ListPreference) findPreference("video_format");
        StreamSettingsSearchAndSummaries.updateCodecSummary(this, codec, codec.getValue(), hdr.isChecked());
        StreamSettingsChrome.stylePreferences(screen, getActivity());
        for (int i = 0; i < screen.getPreferenceCount(); i++) {
            PreferenceCategory category = (PreferenceCategory) screen.getPreference(i);
            List<Preference> preferences = new ArrayList<>();
            allPreferences.put(category.getKey(), category);
            for (int j = 0; j < category.getPreferenceCount(); j++) {
                Preference pref = category.getPreference(j);
                preferences.add(pref);
                allPreferences.put(pref.getKey(), pref);
                homes.put(pref.getKey(), category);
            }
            // A group whose settings this device cannot use is left out entirely.
            if (!preferences.isEmpty()) {
                searchCategories.put(category, preferences);
            }
        }
        // The network rows as the last read of the saved PCs found; onResume reads them again.
        StreamSettingsRowBindings.showKnownPcRows(this);
        refreshState();
        screen.removeAll();
        filterPreferences();
    }
}

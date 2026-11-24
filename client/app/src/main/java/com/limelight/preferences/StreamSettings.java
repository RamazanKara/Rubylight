package com.limelight.preferences;

import android.annotation.SuppressLint;
import android.app.Activity;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.content.res.Configuration;
import android.content.res.XmlResourceParser;
import android.os.Bundle;
import android.preference.Preference;
import android.preference.PreferenceManager;
import android.view.Display;
import android.view.View;
import android.widget.ListView;
import android.widget.TextView;

import androidx.activity.OnBackPressedCallback;
import androidx.appcompat.widget.SearchView;

import com.google.android.material.appbar.MaterialToolbar;
import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.binding.video.GpuDriverStore;
import com.limelight.ui.NightjarActivity;
import com.limelight.ui.ShellBar;
import com.limelight.ui.TopAppBar;
import com.limelight.utils.Dialog;
import com.limelight.utils.UiHelper;

import org.xmlpull.v1.XmlPullParser;
import org.xmlpull.v1.XmlPullParserException;

import java.io.IOException;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * Global streaming settings activity hosting section navigation, two-pane TV/tablet focus
 * coordination, search, and preference panes.
 */
public class StreamSettings extends NightjarActivity {
    /** A TV lays out Settings' screens at {@link com.limelight.ui.AppTheme#TV_COMPACT_PERCENT} of the interface size. */
    @Override
    protected boolean compactOnTelevision() {
        return true;
    }

    static final int EXPORT_LATENCY_REQUEST = 1;
    private static final String STATE_SECTION = "section";
    private static final String STATE_CUSTOMIZE = "customize_open";
    private static final String STATE_MORE_OPEN = "more_open";
    /** Opens this section of {@link #SECTIONS} instead of the list; an unknown name opens the list. */
    public static final String EXTRA_SECTION = "com.limelight.preferences.SECTION";
    /** The key of the setting to scroll to and select once the section shows; see {@link #open}. */
    public static final String EXTRA_FOCUS_KEY = "com.limelight.preferences.FOCUS_KEY";
    /** The Streaming row that leads to a saved PC's connection test and frame pace calibration. */
    static final String NETWORK_TESTS_KEY = "network_tests";
    /** Sharper colored text: Off or Automatic, written into checkbox_yuv444 and experimental_packed_444. */
    static final String SHARP_COLOR_KEY = "sharp_color";
    /** Controllers: Off, One player or A player each (ControllersChoice). */
    static final String CONTROLLERS_KEY = "controllers";
    /** Mouse mode, the switch drawn as Captured or Desktop pointer. */
    static final String MOUSE_MODE_KEY = "checkbox_absolute_mouse_mode";
    /** The main Display arrangement (HostDisplayChoice); a PC's own wins. */
    static final String HOST_DISPLAY_KEY = "host_display_location";
    /** A saved PC's PyroWave frame pace, opened from Debug › Diagnostics. */
    static final String PACE_CALIBRATION_KEY = "pace_calibration";
    static final String LOW_LATENCY_DECODING_KEY = "low_latency_decoding";
    /** The Automatic bitrate switch: stores nothing itself, it shows and sets whether the bitrate is the user's. */
    static final String AUTOMATIC_BITRATE_KEY = "automatic_bitrate";
    /** The row that shows and hides the individual decoding switches under the Low-latency decoding master. */
    static final String LOW_LATENCY_CUSTOMIZE_KEY = "low_latency_customize";
    /** The individual decoding switches, behind Customize. Search still finds them. */
    static final List<String> LOW_LATENCY_CUSTOMIZED = Arrays.asList(
            "checkbox_codec_low_latency", "checkbox_vendor_low_latency", "checkbox_phone_performance_hints",
            "checkbox_codec_performance", PreferenceConfiguration.HEVC_LOW_LATENCY_MODE_PREF_STRING);
    // Top-level screen keys (preferences_root.xml) and the preference categories each one shows. The last one
    // of each holds only its More row (MORE_PREFIX).
    static final Map<String, String[]> SECTIONS = new LinkedHashMap<>();
    static {
        // Each section: its main rows (untitled), its More row, then the folded rows in their groups. Desktop
        // Nightjar draws the same sections, groups and order. "simple" is the simple layer (SimpleProfile): its
        // six rows are all main, and the settings screen shows them before the sections that hold the detail.
        SECTIONS.put("simple", new String[] {"category_simple_main"});
        SECTIONS.put("video", new String[] {"category_video_main", "category_more_video", "category_basic_settings"});
        SECTIONS.put("display", new String[] {"category_display_main", "category_more_display", "category_display_refresh",
                "category_upscaling", "category_host_display"});
        SECTIONS.put("input", new String[] {"category_input_main", "category_more_input", "category_gamepad_settings",
                "category_stream_menu", "category_input_settings", "category_keyboard_shortcuts", "category_onscreen_controls",
                "category_usb_controllers"});
        SECTIONS.put("audio", new String[] {"category_audio_main", "category_more_audio", "category_audio_settings"});
        SECTIONS.put("streaming", new String[] {"category_streaming_main", "category_more_streaming",
                "category_streaming_connection", "category_streaming"});
        SECTIONS.put("app", new String[] {"category_app_main", "category_more_app", "category_ui_settings",
                "category_accessibility"});
        SECTIONS.put("advanced", new String[] {"category_advanced_settings", "category_more_advanced",
                "category_advanced_compatibility", "category_game_info"});
        // Settings whose default is right for nearly everyone, kept for troubleshooting.
        SECTIONS.put("debug", new String[] {"category_debug", "category_more_debug", "category_debug_pyrowave",
                "category_debug_decoder", "category_debug_diagnostics"});
    }
    /**
     * The row ending each section, "more_" and the section: it shows and hides the rows settings_tiers.xml does not
     * list as main. Folded rows keep their key and group, and search, deep links and resets still find them.
     */
    static final String MORE_PREFIX = "more_";
    /**
     * The overview's "More settings" row. The list of sections is simple first: Recommended, Controls, Audio and App
     * show, and this row unfolds {@link #RAIL_FOLDED} in place. It stores no setting.
     */
    static final String RAIL_MORE_KEY = "more_sections";
    /** {@link #RAIL_MORE_KEY}'s name in {@link #moreOpen}; no section has it. */
    static final String RAIL = "rail";
    /** The overview's rows behind {@link #RAIL_MORE_KEY}, in their order: the presets, Profiles and the sections of detail. */
    static final List<String> RAIL_FOLDED = Arrays.asList(
            "stream_presets", "stream_profiles", "video", "display", "streaming", "advanced", "debug");
    /** The sections in the overview's order, which the phone's chips follow: the ones that always show, then the folded ones. */
    static final List<String> RAIL_SECTIONS = Arrays.asList(
            "simple", "input", "audio", "app", "video", "display", "streaming", "advanced", "debug");
    /** The overview's Profiles row, under the presets: profiles hold values from every section. */
    static final String PROFILES_KEY = "stream_profiles";

    final StreamSettingsChrome ui = new StreamSettingsChrome(this);
    final StreamSettingsNavigation nav = new StreamSettingsNavigation(this);
    boolean fromShell;
    String section;
    /** Customize under Low-latency decoding is open: the individual switches show. */
    boolean customizeOpen;
    /** The sections whose More row is open: all their rows show. */
    final Set<String> moreOpen = new HashSet<>();
    /**
     * Rows that do nothing now (SettingRules.WHEN_ROWS) that a search result or a link opened: they show for this
     * visit, so the value they hold can be seen and changed.
     */
    final Set<String> revealed = new HashSet<>();
    /** What this device has that rows act on; tests replace it. */
    SettingDevices devices;
    /** Reads the saved PCs off the UI thread; tests replace it. */
    java.util.concurrent.Executor savedPcReader = task -> new Thread(task, "Saved PCs").start();
    /** What the last read of the saved PCs found: any PC, and any whose PyroWave frame pace can be measured or is saved. */
    boolean anySavedPc;
    boolean anyPacePc;
    /** A saved PC offers more than stock 1400-byte video packets (MaxVideoPacket). */
    boolean anyJumboPc;
    MaterialToolbar toolbar;
    String searchQuery = "";
    SearchView searchView;
    String searchResultKey;
    /** Two panes (sections left, one section right): layout-television and layout-w840dp. */
    boolean twoPane;
    /** The row named by searchResultKey takes focus once selected: a deep link, or a search result picked in the content pane. */
    boolean focusRow;
    String returnSearchQuery;
    TextView contextLabel;
    /** Pending section preview during TV D-pad rail navigation (kept on activity for test inspection). */
    String previewSection;
    /** TV Settings' rail bar and pane motion; null without the TV layout. */
    SettingsRail rail;

    /** Whether the overview shows the rows behind More settings: the user opened it, or this visit landed on one of them. */
    boolean railOpen() {
        return moreOpen.contains(RAIL);
    }

    /** More settings was chosen: the rows below it show or go, and the choice is kept for the next visit. */
    void toggleRail() {
        nav.toggleRail();
    }

    /** The main rows of a section (settings_tiers.xml); every other row of it folds. */
    static List<String> mainKeys(Context context, String section) {
        int tier;
        switch (section) {
            case "simple": tier = R.array.settings_tier_simple; break;
            case "video": tier = R.array.settings_tier_video; break;
            case "display": tier = R.array.settings_tier_display; break;
            case "input": tier = R.array.settings_tier_input; break;
            case "audio": tier = R.array.settings_tier_audio; break;
            case "streaming": tier = R.array.settings_tier_streaming; break;
            case "app": tier = R.array.settings_tier_app; break;
            case "advanced": tier = R.array.settings_tier_advanced; break;
            case "debug": tier = R.array.settings_tier_debug; break;
            default: throw new IllegalArgumentException("No settings section " + section);
        }
        return Arrays.asList(context.getResources().getStringArray(tier));
    }

    /** Whether this row of the section sits behind its More row: not main, not the More row, not behind Customize. */
    static boolean foldsInSection(Context context, String section, String key) {
        return section != null && key != null && !key.equals(MORE_PREFIX + section) &&
                !LOW_LATENCY_CUSTOMIZED.contains(key) && !mainKeys(context, section).contains(key);
    }

    /** Settings at the stream's size, for a start that failed. */
    public static Intent atStreamSize(Context context) {
        return intent(context, null, PreferenceConfiguration.RESOLUTION_PREF_STRING);
    }

    /** The intent for {@link #open}; {@code section} and {@code focusKey} may be null. */
    public static Intent intent(Context context, String section, String focusKey) {
        Intent intent = new Intent(context, StreamSettings.class);
        if (normalizeSection(section) == null && focusKey != null) section = sectionOfKey(context, focusKey);
        if (section != null) intent.putExtra(EXTRA_SECTION, section);
        if (focusKey != null) intent.putExtra(EXTRA_FOCUS_KEY, focusKey);
        return intent;
    }

    /**
     * Opens Settings on {@code section} (a key of {@link #SECTIONS}) with the setting {@code focusKey} selected.
     * Without a (known) section the one that holds the key is used; with neither the list opens. Back returns to
     * the caller.
     */
    public static void open(Context context, String section, String focusKey) {
        Intent intent = intent(context, section, focusKey);
        if (!(context instanceof Activity)) intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        context.startActivity(intent);
    }

    /** The section of the setting with this key in preferences.xml, or null for an unknown key. */
    static String sectionOfKey(Context context, String key) {
        String category = null;
        try (XmlResourceParser parser = context.getResources().getXml(R.xml.preferences)) {
            for (int event = parser.getEventType(); event != XmlPullParser.END_DOCUMENT; event = parser.next()) {
                if (event != XmlPullParser.START_TAG) continue;
                String name = parser.getAttributeValue("http://schemas.android.com/apk/res/android", "key");
                if (name == null) continue;
                if (parser.getName().equals("PreferenceCategory")) category = name;
                if (name.equals(key)) return sectionOf(category);
            }
        } catch (XmlPullParserException | IOException e) {
            LimeLog.warning("Unable to read preference keys: " + e);
        }
        return null;
    }

    static boolean isTelevision(Context context) {
        return (context.getResources().getConfiguration().uiMode & Configuration.UI_MODE_TYPE_MASK)
                == Configuration.UI_MODE_TYPE_TELEVISION;
    }

    /** Whether custom GPU drivers can load here. The debug build's demo mode shows their screen anyway. */
    protected boolean supportsGpuDrivers() {
        return GpuDriverStore.isSupported();
    }

    /** Whether USB controllers can connect here. The debug build's demo mode shows their settings anyway. */
    protected boolean supportsUsbHost() {
        return getPackageManager().hasSystemFeature(PackageManager.FEATURE_USB_HOST);
    }

    void reloadSettings() {
        nav.reloadSettings();
    }

    ListView listOf(int container) {
        return nav.listOf(container);
    }

    ListView sectionList() {
        return nav.sectionList();
    }

    void markSection() {
        nav.markSection();
    }

    static void selectInPlace(ListView list, int position) {
        StreamSettingsNavigation.selectInPlace(list, position);
    }

    String markedRow() {
        return nav.markedRow();
    }

    void focusContentPane(ListView list) {
        nav.focusContentPane(list);
    }

    void previewSection(String key) {
        nav.previewSection(key);
    }

    void sectionRowSelected(String key) {
        nav.sectionRowSelected(key);
    }

    void activateSection(String key, boolean enter) {
        nav.activateSection(key, enter);
    }

    boolean takesFocus(boolean contentPane) {
        return nav.takesFocus(contentPane);
    }

    /**
     * Whether the list of sections shows Debug: Show debug settings is on, one of its settings is changed, or it is
     * the section shown (a link or a search result led there).
     */
    boolean debugListed() {
        return "debug".equals(section) || SettingRules.debugShown(this, PreferenceManager.getDefaultSharedPreferences(this));
    }

    /** What this device has that rows act on (SettingRules.whenApplies). */
    SettingDevices devices() {
        if (devices == null) devices = SettingDevices.of(this);
        return devices;
    }

    /**
     * Shows the search field under the top app bar and focuses it; search stays out of the way until asked for. A search
     * left for a section or by Back comes back as it was, its query and its results.
     */
    public void openSearch() {
        nav.openSearch();
    }

    void setSearchQuery(String query) {
        nav.setSearchQuery(query);
    }

    void openSearchResult(String targetSection, String key) {
        nav.openSearchResult(targetSection, key);
    }

    void rememberMore(String section, boolean open) {
        nav.rememberMore(section, open);
    }

    /** Leaves the overview's More settings open or closed for the next visit, as choosing it does. */
    @androidx.annotation.VisibleForTesting
    static void rememberRail(Context context, boolean open) {
        StreamSettingsNavigation.rememberMore(context, RAIL, open);
    }

    static String normalizeSection(String section) {
        // Restore navigation saved by versions that placed these controls in a separate section.
        if ("developer".equals(section)) return "advanced";
        return section != null && SECTIONS.containsKey(section) ? section : null;
    }

    static int sectionTitle(String section) {
        switch (section) {
            case "simple": return R.string.settings_section_simple;
            case "video": return R.string.settings_section_video;
            case "display": return R.string.settings_section_display;
            case "input": return R.string.settings_section_input;
            case "audio": return R.string.settings_section_audio;
            case "streaming": return R.string.settings_section_streaming;
            case "advanced": return R.string.settings_section_advanced;
            case "debug": return R.string.settings_section_debug;
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
        nav.showSection(newSection);
    }

    static void resetAllSettings(Context context) {
        StreamSettingsChrome.resetAllSettings(context);
    }

    /** Its chrome follows the form: a rotation into another form lays it out again. */
    @Override
    protected boolean followsForm() {
        return true;
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        // The decoder scan the Recommended rows compare against runs on a worker, not here.
        SimpleProfile.warmCapabilities();
        // Framework fragments are recreated by super.onCreate(), before our views exist, and their lists read this.
        twoPane = getResources().getBoolean(R.bool.settings_two_pane);
        moreOpen.addAll(nav.rememberedMore());
        if (savedInstanceState != null) {
            section = normalizeSection(savedInstanceState.getString(STATE_SECTION));
            searchQuery = savedInstanceState.getString("search", "");
            returnSearchQuery = savedInstanceState.getString("return_search");
            nav.returnSearchSection = normalizeSection(savedInstanceState.getString("return_section"));
            nav.lastSearchQuery = savedInstanceState.getString("last_search", "");
            searchResultKey = savedInstanceState.getString("search_result");
            customizeOpen = savedInstanceState.getBoolean(STATE_CUSTOMIZE, false);
            ArrayList<String> open = savedInstanceState.getStringArrayList(STATE_MORE_OPEN);
            if (open != null) moreOpen.addAll(open);
            ArrayList<String> shownRows = savedInstanceState.getStringArrayList("revealed");
            if (shownRows != null) revealed.addAll(shownRows);
        } else {
            String named = getIntent().getStringExtra(EXTRA_SECTION);
            String focusKey = getIntent().getStringExtra(EXTRA_FOCUS_KEY);
            section = normalizeSection(named);
            // A link whose section is missing or unknown finds it from the setting it names.
            String keySection = focusKey == null ? null : sectionOfKey(this, focusKey);
            if (section == null) section = keySection;
            if (focusKey != null && keySection == null) LimeLog.warning("Settings link to an unknown setting: " + focusKey);
            else if (named != null && normalizeSection(named) == null) LimeLog.warning("Settings link to an unknown section: " + named);
            // A deep link names the setting to land on; it only makes sense inside a section.
            if (section != null) searchResultKey = focusKey;
            if (LOW_LATENCY_CUSTOMIZED.contains(focusKey)) customizeOpen = true;
            if (SettingRules.WHEN_ROWS.contains(focusKey)) revealed.add(focusKey);
            nav.unfoldFor(keySection, focusKey);
            nav.enterPane = focusRow = searchResultKey != null;
        }
        super.onCreate(savedInstanceState);

        nav.previousPrefs = PreferenceConfiguration.readPreferences(this);
        nav.startRoot = ShellBar.root(this);
        fromShell = getIntent().getBooleanExtra(ShellBar.EXTRA_FROM_SHELL, false);

        UiHelper.setLocale(this);

        setContentView(R.layout.activity_stream_settings);
        twoPane = findViewById(R.id.settings_sections) != null;
        if (twoPane) {
            getWindow().getDecorView().getViewTreeObserver().addOnTouchModeChangeListener(inTouchMode -> {
                if (!inTouchMode) nav.placeFirstFocus();
            });
        }
        rail = SettingsRail.attach(this);
        nav.dropMisplacedPanes();
        toolbar = TopAppBar.configure(this, R.id.settings_toolbar, getString(R.string.settings), 0);
        TopAppBar.setUpNavigation(toolbar, R.id.settings_back, v -> nav.navigateBack());
        ui.setUpGlass();
        // Search sits at the end of the tab row: Right from the last tab reaches it, also where the bar ends in a wall (TV).
        findViewById(R.id.shell_settings).setNextFocusRightId(R.id.settings_search_button);
        contextLabel = findViewById(R.id.settings_context);
        searchView = findViewById(R.id.settings_search);
        nav.bindSearchView(searchView);
        getOnBackPressedDispatcher().addCallback(this, new OnBackPressedCallback(true) {
            @Override public void handleOnBackPressed() { nav.navigateBack(); }
        });
        ui.updateHeader();

        UiHelper.notifyNewRootView(this);
    }

    @Override
    protected void onSaveInstanceState(Bundle outState) {
        nav.flushPendingSearchBeforeSave();
        super.onSaveInstanceState(outState);
        nav.saveInstanceState(outState, STATE_SECTION, STATE_CUSTOMIZE, STATE_MORE_OPEN);
    }

    @Override
    protected void onPostResume() {
        super.onPostResume();
        nav.stateSaved = false;
        ShellBar.refresh(this);
        if (nav.renderPending) {
            nav.renderPending = false;
            reloadSettings();
        }
    }

    @Override
    protected void onDestroy() {
        Dialog.closeDialogs(this);
        nav.onDestroy();
        super.onDestroy();
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        ui.handleExportLatencyResult(requestCode, resultCode, data);
    }

    @Override
    public void onAttachedToWindow() {
        super.onAttachedToWindow();

        // A recreated two-pane screen gets its section back but not the list of sections, when it came from one pane.
        if (getFragmentManager().findFragmentById(R.id.stream_settings) == null
                || (twoPane && getFragmentManager().findFragmentById(R.id.settings_sections) == null)) {
            reloadSettings();
        } else {
            Display.Mode mode = getWindowManager().getDefaultDisplay().getMode();
            nav.previousDisplayPixelCount = mode.getPhysicalWidth() * mode.getPhysicalHeight();
        }
    }

    @Override
    public void onConfigurationChanged(Configuration newConfig) {
        super.onConfigurationChanged(newConfig);
        if (recreatingForForm()) return;

        // Window resizes (split screen, freeform) cross the two-pane width without recreating the activity.
        if (getResources().getBoolean(R.bool.settings_two_pane) != twoPane) {
            recreate();
            return;
        }

        Display.Mode mode = getWindowManager().getDefaultDisplay().getMode();

        // If the display's physical pixel count has changed, we consider that it's a new display
        // and we should reload our settings (which include display-dependent values).
        //
        // NB: We aren't using displayId here because that stays the same (DEFAULT_DISPLAY) when
        // switching between screens on a foldable device.
        if (mode.getPhysicalWidth() * mode.getPhysicalHeight() != nav.previousDisplayPixelCount) {
            reloadSettings();
        }
    }

    @SuppressLint("MissingSuperCall") // finish() is called below; super would only add the default back handling
    @Override
    // NOTE: This will NOT be called on Android 13+ with android:enableOnBackInvokedCallback="true"
    public void onBackPressed() {
        nav.navigateBack();
    }

    @Override
    public boolean dispatchKeyEvent(android.view.KeyEvent event) {
        if (nav.handlePaneKeyBeforeSuper(event)) return true;
        if (super.dispatchKeyEvent(event)) return true;
        return nav.handlePaneKeyAfterSuper(event);
    }

    static boolean hasRail(Activity activity) {
        return StreamSettingsChrome.hasRail(activity);
    }

    static boolean showsValueAtEnd(Preference preference) {
        return StreamSettingsChrome.showsValueAtEnd(preference);
    }

    static boolean yieldsFirstFocus(Activity activity, View owner) {
        return StreamSettingsChrome.yieldsFirstFocus(activity, owner);
    }

    static CharSequence entryFor(Context context, int names, int values, String value) {
        return StreamSettingsChrome.entryFor(context, names, values, value);
    }

    public static class RootFragment extends SettingsRootFragment {
        static int positionOf(ListView list, String key) {
            return SettingsRootFragment.positionOf(list, key);
        }
    }

    public static class SettingsFragment extends StreamSettingsFragment {
        interface DecoderWidthProbe {
            int probe(Context context);
        }

        // Tests swap in their own probe and wait for the thread.
        static DecoderWidthProbe decoderWidthProbe = StreamSettingsDisplayOptions::probeDecoderWidths;

        static SettingsFragment forSection(String section) {
            SettingsFragment fragment = new SettingsFragment();
            Bundle args = new Bundle();
            args.putString(ARG_SECTION, section);
            fragment.setArguments(args);
            return fragment;
        }

        static String profilesSummary(Context context) {
            return StreamSettingsSearchAndSummaries.profilesSummary(context);
        }

        static boolean fitsPanel(String size, String panel) {
            return StreamSettingsDisplayOptions.fitsPanel(size, panel);
        }

        static void setAutomaticBitrate(SharedPreferences prefs, boolean automatic) {
            StreamSettingsDisplayOptions.setAutomaticBitrate(prefs, automatic);
        }

        static void resetBitrateToDefault(SharedPreferences prefs, String res, String fps, String codec) {
            StreamSettingsDisplayOptions.resetBitrateToDefault(prefs, res, fps, codec);
        }
    }
}

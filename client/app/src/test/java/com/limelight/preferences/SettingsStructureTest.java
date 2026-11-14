package com.limelight.preferences;

import android.app.Application;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.preference.ListPreference;
import android.preference.Preference;
import android.preference.PreferenceCategory;
import android.preference.PreferenceGroup;
import android.preference.PreferenceManager;
import android.preference.PreferenceScreen;
import android.preference.SwitchPreference;
import android.view.View;
import android.widget.FrameLayout;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;
import androidx.appcompat.widget.SearchView;

import com.google.android.material.color.MaterialColors;
import com.limelight.binding.video.DisplayFrameRatePolicy;
import com.limelight.HelpTopicsActivity;
import com.limelight.R;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.shadows.ShadowLooper;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import java.util.concurrent.TimeUnit;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class SettingsStructureTest {
    static void settle(StreamSettings screen) {
        ShadowLooper.idleMainLooper(150, TimeUnit.MILLISECONDS);
        screen.getFragmentManager().executePendingTransactions();
    }

    static android.app.Fragment shown(StreamSettings screen) {
        return screen.getFragmentManager().findFragmentById(R.id.stream_settings);
    }

    /** Opens a section with its More row open, so every row shows (SettingsTiersTest covers the fold). */
    static StreamSettings.SettingsFragment open(StreamSettings screen, String section) {
        screen.moreOpen.add(section);
        screen.showSection(section);
        settle(screen);
        return (StreamSettings.SettingsFragment) shown(screen);
    }

    static List<String> keys(PreferenceGroup group) {
        List<String> keys = new ArrayList<>();
        for (int i = 0; i < group.getPreferenceCount(); i++) keys.add(group.getPreference(i).getKey());
        return keys;
    }

    static StreamSettings.SettingsFragment search(StreamSettings screen, String query) {
        ((SearchView) screen.findViewById(R.id.settings_search)).setQuery(query, false);
        settle(screen);
        return (StreamSettings.SettingsFragment) shown(screen);
    }

    /** The group title above the search result for a setting, or null when the setting is not found. */
    static CharSequence resultGroup(StreamSettings.SettingsFragment fragment, String key) {
        PreferenceScreen results = fragment.getPreferenceScreen();
        for (int i = 0; i < results.getPreferenceCount(); i++) {
            Preference item = results.getPreference(i);
            if (item instanceof PreferenceGroup && ((PreferenceGroup) item).findPreference("search_result_" + key) != null) {
                return item.getTitle();
            }
        }
        return null;
    }

    private static void touchscreen() {
        shadowOf(RuntimeEnvironment.getApplication().getPackageManager())
                .setSystemFeature(PackageManager.FEATURE_TOUCHSCREEN, true);
    }

    private static SharedPreferences prefs() {
        return PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication());
    }

    @Before
    public void aScreenWithArr() {
        DisplayFrameRatePolicy.vrrOfferedOverride = true;
    }

    @After
    public void clear() {
        prefs().edit().clear().commit();
        DisplayFrameRatePolicy.vrrOfferedOverride = null;
    }

    /** Video bitrate and Touch mode wrote their value under the title, the only rows that did. */
    @Test
    public void aRowHoldingOneValueSaysItAtItsEnd() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            settle(screen);
            screen.showSection("video");
            settle(screen);
            StreamSettings.SettingsFragment video = (StreamSettings.SettingsFragment) shown(screen);
            assertEquals(R.layout.settings_preference_value, video.findPreference("seekbar_bitrate_kbps").getLayoutResource());
            screen.showSection("input");
            settle(screen);
            StreamSettings.SettingsFragment input = (StreamSettings.SettingsFragment) shown(screen);
            assertEquals(R.layout.settings_preference_value, input.findPreference("checkbox_touchscreen_trackpad").getLayoutResource());
        }
    }

    @Test
    public void overviewListsTheSectionsInOrder() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            settle(screen);
            PreferenceScreen root = ((StreamSettings.RootFragment) shown(screen)).getPreferenceScreen();
            assertEquals(Arrays.asList("stream_presets", "stream_profiles", "video", "display", "input", "audio", "streaming",
                    "app", "advanced", "debug"), keys(root));
            assertEquals(screen.getString(R.string.settings_section_display), root.findPreference("display").getTitle());
            assertNotNull(root.findPreference("display").getSummary());
            assertNotNull(root.findPreference("audio").getSummary());
        }
    }

    /** nightjar#44: profiles hold values of every section, so they sit with the presets, not in Video. */
    @Test
    public void profilesSitUnderThePresetsAndOpenTheirScreen() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            settle(screen);
            PreferenceScreen root = ((StreamSettings.RootFragment) shown(screen)).getPreferenceScreen();
            Preference profiles = root.findPreference("stream_profiles");
            assertEquals(screen.getString(R.string.profiles_summary_none), profiles.getSummary().toString());
            profiles.getOnPreferenceClickListener().onPreferenceClick(profiles);
            assertEquals(StreamProfilesActivity.class.getName(),
                    shadowOf(screen).getNextStartedActivity().getComponent().getClassName());

            assertNull("not in Video any more", open(screen, "video").findPreference("stream_profiles"));
        }
    }

    @Test
    public void searchFindsTheProfilesAndItsResultOpensThem() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            settle(screen);
            Preference result = search(screen, screen.getString(R.string.profiles_title))
                    .getPreferenceScreen().findPreference("search_result_stream_profiles");
            assertNotNull(result);
            result.getOnPreferenceClickListener().onPreferenceClick(result);
            assertEquals(StreamProfilesActivity.class.getName(),
                    shadowOf(screen).getNextStartedActivity().getComponent().getClassName());
        }
    }

    @Test
    public void groupsNeverRepeatTheirSectionTitle() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            for (String section : StreamSettings.SECTIONS.keySet()) {
                PreferenceScreen shown = open(screen, section).getPreferenceScreen();
                String title = screen.getString(StreamSettings.sectionTitle(section));
                Set<CharSequence> groups = new HashSet<>();
                for (int i = 0; i < shown.getPreferenceCount(); i++) {
                    PreferenceCategory group = (PreferenceCategory) shown.getPreference(i);
                    assertNotEquals(section, title, String.valueOf(group.getTitle()));
                    assertTrue(section + " repeats a group", group.getTitle() == null || groups.add(group.getTitle()));
                    assertTrue(section + " has an empty group", group.getPreferenceCount() > 0);
                }
                // A single group needs no heading of its own.
                if (shown.getPreferenceCount() == 1) assertNull(section, shown.getPreference(0).getTitle());
            }
        }
    }

    @Test
    public void usbControllersAreControlsAndColourIsDisplay() {
        shadowOf(RuntimeEnvironment.getApplication().getPackageManager())
                .setSystemFeature(PackageManager.FEATURE_USB_HOST, true);
        touchscreen();
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            PreferenceScreen controls = open(screen, "input").getPreferenceScreen();
            // The main rows in an untitled first group, then the More row (open here), then the groups behind it.
            assertEquals(Arrays.asList("category_input_main", "category_more_input"), keys(controls).subList(0, 2));
            assertEquals(Arrays.asList("checkbox_touchscreen_trackpad", "checkbox_absolute_mouse_mode",
                    StreamSettings.CONTROLLERS_KEY, "controller_button_mapping", "controller_test",
                    "checkbox_show_onscreen_controls", "checkbox_keyboard_follows_text_fields", "checkbox_clipboard_sync"),
                    keys((PreferenceGroup) controls.getPreference(0)));
            assertEquals(Arrays.asList("category_input_main", "category_more_input", "category_gamepad_settings",
                    "category_stream_menu", "category_input_settings", "category_keyboard_shortcuts",
                    "category_onscreen_controls", "category_usb_controllers"), Arrays.asList(StreamSettings.SECTIONS.get("input")));
            // How the menu opens and what it holds, in one place.
            PreferenceGroup menu = (PreferenceGroup) open(screen, "input").findPreference("category_stream_menu");
            assertEquals(Arrays.asList("list_guide_menu_hold", "checkbox_back_opens_menu", "checkbox_back_as_guide",
                    "checkbox_floating_menu_button", "list_stream_menu_button_y", "list_stream_menu_button_x",
                    "list_stream_menu_button_lb", "list_stream_menu_button_rb", "keyboard_shortcuts_editor"), keys(menu));
            PreferenceGroup usb = (PreferenceGroup) open(screen, "input").findPreference("category_usb_controllers");
            assertEquals(Arrays.asList("checkbox_usb_driver", "checkbox_usb_bind_all", "checkbox_usb_dualsense",
                    "checkbox_usb_switch_pro", "checkbox_usb_forwarding"), keys(usb));
            // DualSense haptics shows under the DualSense switch while that is on.
            prefs().edit().putBoolean("checkbox_usb_dualsense", true).commit();
            usb = (PreferenceGroup) open(screen, "input").findPreference("category_usb_controllers");
            assertEquals(Arrays.asList("checkbox_usb_driver", "checkbox_usb_bind_all", "checkbox_usb_dualsense",
                    "checkbox_usb_dualsense_haptics", "checkbox_usb_switch_pro", "checkbox_usb_forwarding"), keys(usb));
            // Sharper colored text is Video's one row for 4:4:4, native or packed.
            PreferenceGroup video = (PreferenceGroup) open(screen, "video").findPreference("category_basic_settings");
            assertTrue(keys(video).contains(StreamSettings.SHARP_COLOR_KEY));
            assertNull(open(screen, "display").findPreference("checkbox_yuv444"));
            PreferenceScreen advanced = open(screen, "advanced").getPreferenceScreen();
            // The engine leads Advanced's main rows, then the decoding switches; the GPU rows show only on a GPU that has them.
            assertEquals(Arrays.asList(PreferenceConfiguration.RUST_ENGINE_PREF_STRING, StreamSettings.LOW_LATENCY_DECODING_KEY,
                    StreamSettings.LOW_LATENCY_CUSTOMIZE_KEY, "checkbox_texture_view", "checkbox_vulkan_render"),
                    keys((PreferenceGroup) advanced.getPreference(0)));
        }
    }

    @Test
    public void searchNamesTheSectionOnceAndTheGroupWhenThereIsOne() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            StreamSettings.SettingsFragment results = search(screen, "language");
            assertEquals(screen.getString(R.string.settings_section_app), resultGroup(results, "list_languages").toString());
            // Sharpness only exists while a sharp upscaler is chosen.
            assertNull(resultGroup(search(screen, "sharpness"), "upscaling_sharpness"));
            ((SearchView) screen.findViewById(R.id.settings_search)).setQuery("", false);
            settle(screen);
            prefs().edit().putString("upscaling_mode", "fsr1").commit();
            results = search(screen, "sharpness");
            assertEquals(screen.getString(R.string.settings_section_display) + " · " + screen.getString(R.string.settings_category_image),
                    resultGroup(results, "upscaling_sharpness").toString());
            // Words from the explanation behind a switch's info button still find it.
            assertNotNull(resultGroup(search(screen, "equalizers"), "checkbox_enable_audiofx"));
        }
    }

    @Test
    public void automaticQualityMinimumsFollowTheMode() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            PreferenceScreen video = open(controller.get(), "video").getPreferenceScreen();
            ListPreference mode = (ListPreference) video.findPreference("adaptive_quality_mode");
            assertEquals("off", mode.getValue());
            assertNull(video.findPreference("adaptive_quality_min_resolution"));
            assertNull(video.findPreference("adaptive_quality_min_fps"));
            mode.setValue("motion");
            assertTrue(video.findPreference("adaptive_quality_min_resolution").isEnabled());
            assertTrue(video.findPreference("adaptive_quality_min_fps").isEnabled());
            mode.setValue("off");
            assertNull(video.findPreference("adaptive_quality_min_resolution"));
            assertNull(video.findPreference("adaptive_quality_min_fps"));
        }
        // A stored mode shows them when the screen opens.
        prefs().edit().putString("adaptive_quality_mode", "detail").commit();
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            assertTrue(open(controller.get(), "video").getPreferenceScreen().findPreference("adaptive_quality_min_fps").isEnabled());
        }
    }

    private static void mobileData(boolean has) {
        shadowOf(RuntimeEnvironment.getApplication().getPackageManager())
                .setSystemFeature(PackageManager.FEATURE_TELEPHONY, has);
    }

    @Test
    public void mobileDataRowsShowOnlyWithMobileDataOrAValueThatStillActs() {
        // A TV or a Wi-Fi tablet: no mobile data rows, in their sections or in search.
        mobileData(false);
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            assertNull(resultGroup(search(screen, "mobile data"), "list_mobile_bitrate_kbps"));
            ((SearchView) screen.findViewById(R.id.settings_search)).setQuery("", false);
            settle(screen);
            assertNull(open(screen, "video").getPreferenceScreen().findPreference("list_mobile_bitrate_kbps"));
            PreferenceScreen streaming = open(screen, "streaming").getPreferenceScreen();
            assertNotNull(streaming.findPreference("checkbox_client_roaming"));
            assertNull(streaming.findPreference("checkbox_roaming_metered"));
        }
        // A value set earlier still acts on a metered Wi-Fi, so its row stays to undo it.
        prefs().edit().putString("list_mobile_bitrate_kbps", "10000").putBoolean("checkbox_roaming_metered", true).commit();
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            assertNotNull(open(screen, "video").getPreferenceScreen().findPreference("list_mobile_bitrate_kbps"));
            assertNotNull(open(screen, "streaming").getPreferenceScreen().findPreference("checkbox_roaming_metered"));
        }
        prefs().edit().clear().commit();
        mobileData(true);
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            assertNotNull(resultGroup(search(screen, "mobile data"), "list_mobile_bitrate_kbps"));
            ((SearchView) screen.findViewById(R.id.settings_search)).setQuery("", false);
            settle(screen);
            assertNotNull(open(screen, "video").getPreferenceScreen().findPreference("list_mobile_bitrate_kbps"));
            assertNotNull(open(screen, "streaming").getPreferenceScreen().findPreference("checkbox_roaming_metered"));
        }
    }

    @Test
    public void mobileDataShowsOnlyWhileClientRoamingIsOn() {
        mobileData(true);
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            PreferenceScreen streaming = open(controller.get(), "streaming").getPreferenceScreen();
            SwitchPreference roaming = (SwitchPreference) streaming.findPreference("checkbox_client_roaming");
            assertTrue(roaming.isChecked());
            SwitchPreference metered = (SwitchPreference) streaming.findPreference("checkbox_roaming_metered");
            assertFalse(metered.isChecked());
            roaming.setChecked(false);
            assertNull(streaming.findPreference("checkbox_roaming_metered"));
            roaming.setChecked(true);
            assertNotNull(streaming.findPreference("checkbox_roaming_metered"));
        }
        // The C engine does not roam: both rows are hidden.
        prefs().edit().putBoolean("checkbox_rust_engine", false).commit();
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            PreferenceScreen streaming = open(controller.get(), "streaming").getPreferenceScreen();
            assertNull(streaming.findPreference("checkbox_client_roaming"));
            assertNull(streaming.findPreference("checkbox_roaming_metered"));
        }
    }

    @Test
    public void playOverTheInternetIsOnInConnectionForBothEngines() {
        prefs().edit().putBoolean("checkbox_rust_engine", false).commit();
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            assertNotNull(resultGroup(search(screen, "Play over the internet"), "checkbox_internet_play"));
            ((SearchView) screen.findViewById(R.id.settings_search)).setQuery("", false);
            settle(screen);
            PreferenceGroup connection = (PreferenceGroup) open(screen, "streaming").getPreferenceScreen()
                    .findPreference("category_streaming_connection");
            assertTrue(((SwitchPreference) connection.findPreference("checkbox_internet_play")).isChecked());
        }
    }

    @Test
    public void groupingIncomingVideoPacketsIsOnInConnectionWithTheRustEngineOnly() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            assertNotNull(resultGroup(search(screen, "Group incoming video packets"), "checkbox_receive_coalescing"));
            ((SearchView) screen.findViewById(R.id.settings_search)).setQuery("", false);
            settle(screen);
            PreferenceGroup connection = (PreferenceGroup) open(screen, "streaming").getPreferenceScreen()
                    .findPreference("category_streaming_connection");
            assertTrue(keys(connection).contains("checkbox_receive_coalescing"));
            assertTrue(((SwitchPreference) connection.findPreference("checkbox_receive_coalescing")).isChecked());
        }
        // The C engine has its own receive path: the row is hidden.
        prefs().edit().putBoolean("checkbox_rust_engine", false).commit();
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            PreferenceScreen streaming = open(controller.get(), "streaming").getPreferenceScreen();
            assertNull(streaming.findPreference("checkbox_receive_coalescing"));
        }
    }

    @Test
    public void usbForwardingIsOffAndShowsOnlyWithTheRustEngine() {
        shadowOf(RuntimeEnvironment.getApplication().getPackageManager())
                .setSystemFeature(PackageManager.FEATURE_USB_HOST, true);
        touchscreen();
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            PreferenceScreen controls = open(controller.get(), "input").getPreferenceScreen();
            SwitchPreference forwarding = (SwitchPreference) controls.findPreference("checkbox_usb_forwarding");
            assertFalse(forwarding.isChecked());
            assertEquals("Forward USB devices", String.valueOf(forwarding.getTitle()));
        }
        // The C engine forwards nothing: the row is hidden.
        prefs().edit().putBoolean("checkbox_rust_engine", false).commit();
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            assertNull(open(controller.get(), "input").getPreferenceScreen().findPreference("checkbox_usb_forwarding"));
        }
    }

    // Moonlight V+'s clipboard sync is an established feature there: off until asked, but no Experimental label.
    @Test
    public void clipboardSyncIsOffAndShowsOnlyWithTheRustEngine() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            PreferenceScreen input = open(controller.get(), "input").getPreferenceScreen();
            SwitchPreference sync = (SwitchPreference) input.findPreference("checkbox_clipboard_sync");
            assertFalse(sync.isChecked());
            assertEquals(controller.get().getString(R.string.title_clipboard_sync), String.valueOf(sync.getTitle()));
            assertFalse(String.valueOf(sync.getTitle()).contains("Experimental"));
        }
        // The C engine has no clipboard sync: the row is hidden.
        prefs().edit().putBoolean("checkbox_rust_engine", false).commit();
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            assertNull(open(controller.get(), "input").getPreferenceScreen().findPreference("checkbox_clipboard_sync"));
        }
    }

    @Test
    public void adaptiveRefreshHidesFramePacingAndTheLowerRefreshRate() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            // Rows that do not apply are hidden, not greyed (SettingRules), and come back in place.
            PreferenceScreen display = open(controller.get(), "display").getPreferenceScreen();
            android.preference.ListPreference vrr = (android.preference.ListPreference) display.findPreference("checkbox_vrr");
            assertNotNull(display.findPreference("frame_pacing"));
            assertNotNull(display.findPreference("checkbox_reduce_refresh_rate"));
            for (String on : new String[] {"on", "fastest"}) {
                vrr.setValue(on);
                assertNull(on, display.findPreference("frame_pacing"));
                assertNull(on, display.findPreference("checkbox_reduce_refresh_rate"));
                vrr.setValue("off");
                assertNotNull(on, display.findPreference("frame_pacing"));
                assertNotNull(on, display.findPreference("checkbox_reduce_refresh_rate"));
            }
        }
    }

    /** Variable refresh rate is one row of Off, On and Fastest over checkbox_vrr and checkbox_real_vrr. */
    @Test
    public void variableRefreshRateIsOneRowOverBothSwitches() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings.SettingsFragment fragment = open(controller.get(), "display");
            assertNull("no row of its own for Real VRR", fragment.findPreference("checkbox_real_vrr"));
            android.preference.ListPreference vrr = (android.preference.ListPreference) fragment.findPreference("checkbox_vrr");
            assertNotNull(vrr);
            String[] names = controller.get().getResources().getStringArray(R.array.vrr_choice_names);
            assertEquals(3, vrr.getEntries().length);
            assertEquals("off", vrr.getValue());
            assertEquals(names[0], String.valueOf(vrr.getEntry()));
            assertTrue("Real VRR keeps its default until a choice writes it", prefs().getBoolean("checkbox_real_vrr", true));

            // Each choice writes exactly its two values; Off leaves Real VRR as it was.
            vrr.setValue("fastest");
            assertTrue(prefs().getBoolean("checkbox_vrr", false));
            assertFalse(prefs().getBoolean("checkbox_real_vrr", true));
            vrr.setValue("off");
            assertFalse(prefs().getBoolean("checkbox_vrr", true));
            assertFalse("Off keeps Fastest's Real VRR", prefs().getBoolean("checkbox_real_vrr", true));
            vrr.setValue("on");
            assertTrue(prefs().getBoolean("checkbox_vrr", false));
            assertTrue(prefs().getBoolean("checkbox_real_vrr", false));
            vrr.setValue("off");
            assertTrue("Off keeps On's Real VRR", prefs().getBoolean("checkbox_real_vrr", false));
            assertFalse("no string is stored under the row's key", prefs().getAll().get("checkbox_vrr") instanceof String);

            // The row follows the switches when something else writes them, such as a preset.
            prefs().edit().putBoolean("checkbox_vrr", true).putBoolean("checkbox_real_vrr", false).commit();
            assertEquals("fastest", vrr.getValue());
            assertEquals(names[2], String.valueOf(vrr.getEntry()));
            prefs().edit().putBoolean("checkbox_real_vrr", true).commit();
            assertEquals(names[1], String.valueOf(vrr.getEntry()));
            // The last value the list saw still writes when chosen again after an outside change.
            vrr.setValue("on");
            prefs().edit().putBoolean("checkbox_vrr", false).commit();
            assertEquals("off", vrr.getValue());
            vrr.setValue("on");
            assertTrue(prefs().getBoolean("checkbox_vrr", false));
        }
    }

    @Test
    public void lowLatencyDecodingSetsItsPartsAndSaysWhenOneIsOff() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            StreamSettings.SettingsFragment advanced = open(screen, "advanced");
            Preference customize = advanced.findPreference(StreamSettings.LOW_LATENCY_CUSTOMIZE_KEY);
            customize.getOnPreferenceClickListener().onPreferenceClick(customize);
            SwitchPreference merged = (SwitchPreference) advanced.findPreference(StreamSettings.LOW_LATENCY_DECODING_KEY);
            assertTrue(merged.isChecked());
            assertNull(merged.getSummary());

            prefs().edit().putBoolean("checkbox_vendor_low_latency", false).commit();
            assertFalse(((SwitchPreference) advanced.findPreference("checkbox_vendor_low_latency")).isChecked());
            assertFalse(merged.isChecked());
            assertEquals(screen.getString(R.string.settings_low_latency_partial), merged.getSummary().toString());

            assertTrue(merged.getOnPreferenceChangeListener().onPreferenceChange(merged, true));
            for (String key : PreferenceConfiguration.LOW_LATENCY_DECODING_KEYS) {
                assertTrue(key, prefs().getBoolean(key, false));
                assertTrue(key, ((SwitchPreference) advanced.findPreference(key)).isChecked());
            }
            assertTrue(merged.isChecked());
            assertNull(merged.getSummary());

            assertTrue(merged.getOnPreferenceChangeListener().onPreferenceChange(merged, false));
            for (String key : PreferenceConfiguration.LOW_LATENCY_DECODING_KEYS) {
                assertFalse(key, prefs().getBoolean(key, true));
                assertFalse(key, ((SwitchPreference) advanced.findPreference(key)).isChecked());
            }
            assertFalse(merged.isChecked());
            assertNull(merged.getSummary());
            // Nothing is stored under the merged switch's own key.
            assertFalse(prefs().contains(StreamSettings.LOW_LATENCY_DECODING_KEY));
        }
    }

    @Test
    public void lowLatencyDecodingSaysPartlyOffOnTheTvWhereAndroidsModeStartsOff() {
        CodecLowLatencyCorruptionTest.device("AFTNE4E43F", "c2.mtk.hevc.decoder");
        com.limelight.binding.video.MediaCodecHelper.forgetCodecList();
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            SwitchPreference master = (SwitchPreference) open(screen, "advanced").findPreference(StreamSettings.LOW_LATENCY_DECODING_KEY);
            // The vendor keys and hints are still on: neither a plain Off nor On would be true.
            assertFalse(master.isChecked());
            assertEquals(screen.getString(R.string.settings_low_latency_partial), master.getSummary().toString());

            // The master switch is how the user forces it back on, and that holds.
            assertTrue(master.getOnPreferenceChangeListener().onPreferenceChange(master, true));
            assertTrue(master.isChecked());
            assertNull(master.getSummary());
            assertTrue(PreferenceConfiguration.readPreferences(screen).codecLowLatency);
        } finally {
            com.limelight.binding.video.MediaCodecHelper.forgetCodecList();
        }
    }

    @Test
    public void hevcOverrideShowsPartialLatencyWithoutBeingClearedByTheMasterSwitch() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            StreamSettings.SettingsFragment advanced = open(screen, "advanced");
            Preference customize = advanced.findPreference(StreamSettings.LOW_LATENCY_CUSTOMIZE_KEY);
            customize.getOnPreferenceClickListener().onPreferenceClick(customize);
            SwitchPreference master = (SwitchPreference) advanced.findPreference(StreamSettings.LOW_LATENCY_DECODING_KEY);
            // Off stores the older switch's "disable" choice.
            android.preference.ListPreference override = (android.preference.ListPreference) advanced.findPreference(
                    PreferenceConfiguration.HEVC_LOW_LATENCY_MODE_PREF_STRING);
            override.setValue("on");
            assertNull(master.getSummary());
            override.setValue("off");
            assertEquals(screen.getString(R.string.settings_low_latency_partial), master.getSummary());
            master.getOnPreferenceChangeListener().onPreferenceChange(master, false);
            assertNull(master.getSummary());
            master.getOnPreferenceChangeListener().onPreferenceChange(master, true);
            assertEquals(screen.getString(R.string.settings_low_latency_partial), master.getSummary());
            assertEquals("off", override.getValue());
            assertTrue(prefs().getBoolean(PreferenceConfiguration.DISABLE_HEVC_LOW_LATENCY_PREF_STRING, false));
            override.setValue("auto");
            assertNull(master.getSummary());
        }
    }

    @Test
    public void gpuDriverIsHiddenWithoutAnArm64AdrenoGpu() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings.SettingsFragment advanced = open(controller.get(), "advanced");
            assertNull(advanced.getPreferenceScreen().findPreference("gpu_driver"));
            assertNull(resultGroup(search(controller.get(), "Snapdragon"), "gpu_driver"));
        }
    }

    @Test
    @Config(sdk = 30, shadows = {com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
            GpuDriverSettingsTest.SupportedDriverStore.class},
            instrumentedPackages = {"com.limelight.nvstream.jni", "com.limelight.binding.video"})
    public void gpuDriverIsShownOnArm64AdrenoDevices() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            Preference driver = open(screen, "advanced").getPreferenceScreen().findPreference("gpu_driver");
            assertNotNull(driver);
            assertEquals(screen.getString(R.string.gpu_driver_system), driver.getSummary().toString());
            // It is an expert option now, no longer next to upscaling.
            assertNull(open(screen, "display").getPreferenceScreen().findPreference("gpu_driver"));
        }
    }

    @Test
    public void switchRowsShowNoOnOffTextAndDisabledRowsLookDisabled() {
        touchscreen();
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            StreamSettings.SettingsFragment display = open(screen, "display");
            FrameLayout parent = new FrameLayout(screen);
            SwitchPreference color = (SwitchPreference) open(screen, "video").findPreference("checkbox_enable_sdr_10bit");
            View row = color.getView(null, parent);
            assertEquals(View.GONE, row.findViewById(android.R.id.summary).getVisibility());
            assertEquals(View.VISIBLE, row.findViewById(R.id.preference_info).getVisibility());
            int enabledTitle = ((TextView) row.findViewById(android.R.id.title)).getCurrentTextColor();

            // Rows that do not apply are hidden now; a row greyed for another reason (old SHIELD HDR) looks disabled.
            Preference lower = display.findPreference("checkbox_reduce_refresh_rate");
            lower.setEnabled(false);
            View disabled = lower.getView(null, parent);
            assertFalse(disabled.isEnabled());
            assertNotEquals(enabledTitle, ((TextView) disabled.findViewById(android.R.id.title)).getCurrentTextColor());
            // Its explanation still opens, to say why it is unavailable.
            View info = disabled.findViewById(R.id.preference_info);
            assertTrue(info.isEnabled());
            info.performClick();
            AlertDialog explanation = (AlertDialog) ShadowDialog.getLatestDialog();
            assertTrue(explanation.isShowing());
            explanation.dismiss();

            // A host-dependent switch says so briefly.
            TextView hostLine = display.findPreference("checkbox_virtual_display").getView(null, parent)
                    .findViewById(android.R.id.summary);
            assertEquals(View.VISIBLE, hostLine.getVisibility());
            assertEquals(screen.getString(R.string.settings_needs_host_support), hostLine.getText().toString());
        }
    }

    @Test
    public void resetAsksWithADestructiveConfirmation() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            Preference reset = open(screen, "app").findPreference("reset_all");
            reset.getOnPreferenceClickListener().onPreferenceClick(reset);
            AlertDialog confirm = (AlertDialog) ShadowDialog.getLatestDialog();
            assertTrue(confirm.isShowing());
            // The one red control: light text on the danger capsule.
            assertEquals(androidx.core.content.ContextCompat.getColor(confirm.getContext(), R.color.nj_color_on_danger_fill),
                    confirm.getButton(AlertDialog.BUTTON_POSITIVE).getCurrentTextColor());
            assertEquals(R.drawable.dialog_button_danger, shadowOf(confirm.getButton(AlertDialog.BUTTON_POSITIVE).getBackground())
                    .getCreatedFromResId());
            confirm.getButton(AlertDialog.BUTTON_NEGATIVE).performClick();
        }
    }

    @Test
    public void helpOpensTheBundledPage() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            Preference help = open(screen, "app").findPreference("help");
            help.getOnPreferenceClickListener().onPreferenceClick(help);
            assertEquals(HelpTopicsActivity.class.getName(),
                    shadowOf(screen).getNextStartedActivity().getComponent().getClassName());
        }
    }

    @Test
    public void touchModeHelpKeepsThePickerOpen() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            Preference touch = open(screen, "input").findPreference("checkbox_touchscreen_trackpad");
            assertEquals(screen.getResources().getStringArray(R.array.stream_touch_modes)[0], touch.getSummary().toString());
            touch.getOnPreferenceClickListener().onPreferenceClick(touch);
            AlertDialog picker = (AlertDialog) ShadowDialog.getLatestDialog();
            picker.getButton(AlertDialog.BUTTON_NEUTRAL).performClick();
            ShadowLooper.idleMainLooper();
            AlertDialog help = (AlertDialog) ShadowDialog.getLatestDialog();
            assertNotSame(picker, help);
            assertTrue(picker.isShowing());
            assertEquals(screen.getString(R.string.settings_touch_mode_help),
                    ((TextView) help.findViewById(android.R.id.message)).getText().toString());
            help.dismiss();
            picker.dismiss();
        }
    }
}

package com.limelight.preferences;

import android.app.Application;
import android.content.SharedPreferences;
import android.preference.ListPreference;
import android.preference.Preference;
import android.preference.PreferenceGroup;
import android.preference.PreferenceManager;
import android.preference.PreferenceScreen;
import android.preference.SwitchPreference;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;

import com.limelight.R;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.shadows.ShadowToast;

import static com.limelight.preferences.SettingsStructureTest.open;
import static com.limelight.preferences.SettingsStructureTest.resultGroup;
import static com.limelight.preferences.SettingsStructureTest.search;
import static com.limelight.preferences.SettingsStructureTest.settle;
import static com.limelight.preferences.SettingsStructureTest.shown;
import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class DeveloperOptionsTest {
    private static SharedPreferences prefs() {
        return PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication());
    }

    @After
    public void clear() {
        prefs().edit().clear().commit();
    }

    private static PreferenceScreen root(StreamSettings screen) {
        screen.showSection(null);
        settle(screen);
        return ((StreamSettings.RootFragment) shown(screen)).getPreferenceScreen();
    }

    @Test
    public void hiddenUntilTheVersionInAboutIsTappedSevenTimes() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            assertNull(root(screen).findPreference(StreamSettings.DEVELOPER_SECTION));
            assertNull(resultGroup(search(screen, "compatibility"), "checkbox_texture_view"));
            ((androidx.appcompat.widget.SearchView) screen.findViewById(R.id.settings_search)).setQuery("", false);
            settle(screen);
            // An old saved section cannot open the hidden screen.
            screen.showSection(StreamSettings.DEVELOPER_SECTION);
            settle(screen);
            assertTrue(shown(screen) instanceof StreamSettings.RootFragment);

            Preference about = open(screen, "app").findPreference("about_app");
            about.getOnPreferenceClickListener().onPreferenceClick(about);
            AlertDialog dialog = (AlertDialog) ShadowDialog.getLatestDialog();
            TextView version = dialog.findViewById(android.R.id.message);
            assertTrue(version.getText().toString().contains(com.limelight.BuildConfig.VERSION_NAME));
            for (int tap = 1; tap < StreamSettings.DEVELOPER_TAPS; tap++) {
                version.performClick();
                assertFalse(prefs().getBoolean(StreamSettings.DEVELOPER_OPTIONS_PREF_STRING, false));
            }
            assertEquals(screen.getResources().getQuantityString(R.plurals.developer_options_taps_left, 1, 1),
                    ShadowToast.getTextOfLatestToast());
            version.performClick();
            assertTrue(prefs().getBoolean(StreamSettings.DEVELOPER_OPTIONS_PREF_STRING, false));
            assertEquals(screen.getString(R.string.developer_options_on), ShadowToast.getTextOfLatestToast());
            version.performClick();
            assertEquals(screen.getString(R.string.developer_options_already_on), ShadowToast.getTextOfLatestToast());
            dialog.dismiss();

            PreferenceScreen root = root(screen);
            assertNotNull(root.findPreference(StreamSettings.DEVELOPER_SECTION));
            assertEquals(StreamSettings.DEVELOPER_SECTION, root.getPreference(root.getPreferenceCount() - 1).getKey());
            assertEquals(screen.getString(R.string.settings_section_developer) + " · " +
                            screen.getString(R.string.settings_category_decoder),
                    resultGroup(search(screen, "compatibility"), "checkbox_texture_view").toString());
        }
    }

    @Test
    public void developerOptionsHoldTheTestSwitchesAndDiagnostics() {
        prefs().edit().putBoolean(StreamSettings.DEVELOPER_OPTIONS_PREF_STRING, true).commit();
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings.SettingsFragment developer = open(controller.get(), StreamSettings.DEVELOPER_SECTION);
            for (String key : new String[] {"checkbox_codec_low_latency", "checkbox_vendor_low_latency",
                    "checkbox_phone_performance_hints", "checkbox_codec_performance", "checkbox_drop_late_frames",
                    "checkbox_unbatched_input", "checkbox_network_priority", "checkbox_texture_view", "checkbox_unlock_fps",
                    "export_latency_csv", "latency_benchmark", "checkbox_enable_post_stream_toast"}) {
                assertNotNull(key, developer.getPreferenceScreen().findPreference(key));
            }
            // None of them is left at the top level of another section.
            for (String section : new String[] {"video", "display", "input", "audio", "streaming", "app", "advanced"}) {
                PreferenceScreen other = open(controller.get(), section).getPreferenceScreen();
                assertNull(section, other.findPreference("checkbox_texture_view"));
                assertNull(section, other.findPreference("export_latency_csv"));
            }
            // The overlay mode is a normal choice: it sits in Streaming, not here.
            assertNull(developer.getPreferenceScreen().findPreference("performance_overlay_mode"));
        }
    }

    private static List<String> overlayModes(StreamSettings screen) {
        ListPreference mode = (ListPreference) open(screen, "streaming").findPreference("performance_overlay_mode");
        List<String> values = new ArrayList<>();
        for (CharSequence value : mode.getEntryValues()) values.add(value.toString());
        return values;
    }

    @Test
    public void theOverlayModeSitsBesideTheOverlaySwitchAndOffersDebugToDevelopers() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            PreferenceGroup streaming = (PreferenceGroup) open(screen, "streaming").getPreferenceScreen().getPreference(0);
            List<String> keys = SettingsStructureTest.keys(streaming);
            assertEquals(keys.indexOf("checkbox_enable_perf_overlay") + 1, keys.indexOf("performance_overlay_mode"));
            assertEquals(Arrays.asList("compact", "statistics"), overlayModes(screen));
            ListPreference mode = (ListPreference) open(screen, "streaming").findPreference("performance_overlay_mode");
            assertEquals(screen.getString(R.string.overlay_compact), mode.getEntry().toString());

            prefs().edit().putBoolean(StreamSettings.DEVELOPER_OPTIONS_PREF_STRING, true).commit();
            assertEquals(Arrays.asList("compact", "statistics", "debug"), overlayModes(screen));
        }
    }

    @Test
    public void aSavedDebugOverlayKeepsWorkingWhileDeveloperOptionsAreHidden() {
        prefs().edit().putString("performance_overlay_mode", "debug").commit();
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            assertEquals(Arrays.asList("compact", "statistics", "debug"), overlayModes(screen));
            ListPreference mode = (ListPreference) open(screen, "streaming").findPreference("performance_overlay_mode");
            assertEquals(screen.getString(R.string.overlay_debug), mode.getEntry().toString());
            assertEquals("debug", prefs().getString("performance_overlay_mode", null));
            assertEquals("debug", com.limelight.binding.video.PerformanceOverlay.normalizeMode(
                    prefs().getString("performance_overlay_mode", null)));
        }
    }

    @Test
    public void hidingThemAgainKeepsWhatWasChanged() {
        prefs().edit().putBoolean(StreamSettings.DEVELOPER_OPTIONS_PREF_STRING, true)
                // An install past the one-time native-defaults migration, which turns latency switches on.
                .putInt(PreferenceConfiguration.PREFS_VERSION_PREF_STRING, PreferenceConfiguration.PROFILE_NATIVE_VERSION)
                .putBoolean("checkbox_texture_view", true)
                .putBoolean("checkbox_network_priority", false)
                .putString("performance_overlay_mode", "debug").commit();
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            SwitchPreference show = (SwitchPreference) open(screen, StreamSettings.DEVELOPER_SECTION)
                    .findPreference(StreamSettings.DEVELOPER_OPTIONS_PREF_STRING);
            assertTrue(show.isChecked());
            if (show.getOnPreferenceChangeListener().onPreferenceChange(show, false)) show.setChecked(false);
            ShadowLooper.idleMainLooper();
            settle(screen);
            assertTrue(shown(screen) instanceof StreamSettings.RootFragment);
            assertEquals(screen.getString(R.string.developer_options_hidden), ShadowToast.getTextOfLatestToast());
            assertFalse(prefs().getBoolean(StreamSettings.DEVELOPER_OPTIONS_PREF_STRING, true));
            assertNull(root(screen).findPreference(StreamSettings.DEVELOPER_SECTION));

            PreferenceConfiguration config = PreferenceConfiguration.readPreferences(screen);
            assertTrue(config.useTextureView);
            assertFalse(config.networkPriority);
            assertEquals("debug", prefs().getString("performance_overlay_mode", null));
        }
    }

    @Test
    public void resettingAllSettingsHidesThemAgain() {
        prefs().edit().putBoolean(StreamSettings.DEVELOPER_OPTIONS_PREF_STRING, true).commit();
        StreamSettings.resetAllSettings(RuntimeEnvironment.getApplication());
        assertFalse(StreamSettings.developerOptionsShown(RuntimeEnvironment.getApplication()));
    }
}

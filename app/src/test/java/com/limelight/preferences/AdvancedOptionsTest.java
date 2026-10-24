package com.limelight.preferences;

import android.app.Application;
import android.content.SharedPreferences;
import android.os.Bundle;
import android.preference.ListPreference;
import android.preference.Preference;
import android.preference.PreferenceGroup;
import android.preference.PreferenceManager;
import android.preference.PreferenceScreen;
import android.preference.SwitchPreference;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;

import com.google.android.material.appbar.MaterialToolbar;
import com.limelight.R;
import com.limelight.binding.video.PyroWaveDecoderRenderer;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.shadows.ShadowDialog;

import static com.limelight.preferences.SettingsStructureTest.open;
import static com.limelight.preferences.SettingsStructureTest.resultGroup;
import static com.limelight.preferences.SettingsStructureTest.search;
import static com.limelight.preferences.SettingsStructureTest.settle;
import static com.limelight.preferences.SettingsStructureTest.shown;
import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, qualifiers = "en", application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = {"com.limelight.nvstream.jni", "com.limelight.binding.video"})
public class AdvancedOptionsTest {
    private static final String LEGACY_UNLOCK = "developer_options";
    private static final String GPU_BOOST = PreferenceConfiguration.GPU_MAX_CLOCKS_PREF_STRING;
    private static final String HEVC = PreferenceConfiguration.DISABLE_HEVC_LOW_LATENCY_PREF_STRING;
    /** The controls Advanced shows without any further tap. The individual decoding switches sit behind Customize. */
    private static final String[] ADVANCED_CONTROLS = {
            "low_latency_customize", "checkbox_unbatched_input",
            "checkbox_network_priority", "checkbox_texture_view", "checkbox_gl_render", "checkbox_full_range",
            "checkbox_enable_sops", "export_latency_csv", "latency_benchmark", "checkbox_enable_post_stream_toast",
            "checkbox_rust_engine"
    };

    /** Opens Customize under Low-latency decoding, once. */
    private static void openCustomize(PreferenceScreen advanced) {
        if (advanced.findPreference(StreamSettings.LOW_LATENCY_CUSTOMIZED.get(0)) != null) return;
        Preference customize = advanced.findPreference("low_latency_customize");
        customize.getOnPreferenceClickListener().onPreferenceClick(customize);
    }

    @Implements(value = PyroWaveDecoderRenderer.class, isInAndroidSdk = false)
    public static class NonAdrenoGpu {
        @Implementation protected static boolean hasAdrenoGpu() { return false; }
    }

    @Implements(value = PyroWaveDecoderRenderer.class, isInAndroidSdk = false)
    public static class AdrenoGpu {
        @Implementation protected static boolean hasAdrenoGpu() { return true; }
    }

    private static SharedPreferences prefs() {
        return PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication());
    }

    @After
    public void clear() {
        prefs().edit().clear().commit();
    }

    @Test
    public void hevcLowLatencyOptOutIsOffByDefaultAndIndependentOfTheGeneralSwitch() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            PreferenceScreen advanced = open(controller.get(), "advanced").getPreferenceScreen();
            openCustomize(advanced);
            // The switch reads "use the flags" and stores the older "disable" choice.
            SwitchPreference hevc = (SwitchPreference) advanced.findPreference(HEVC);
            assertTrue(hevc.isChecked());
            hevc.setChecked(false);
            assertTrue(PreferenceConfiguration.readPreferences(controller.get()).disableHevcLowLatency);
            for (boolean general : new boolean[] {false, true}) {
                PreferenceConfiguration.setLowLatencyDecoding(prefs(), general);
                assertTrue(PreferenceConfiguration.readPreferences(controller.get()).disableHevcLowLatency);
            }
            hevc.setChecked(true);
            assertFalse(PreferenceConfiguration.readPreferences(controller.get()).disableHevcLowLatency);
        }
    }

    private static PreferenceScreen root(StreamSettings screen) {
        screen.showSection(null);
        settle(screen);
        return ((StreamSettings.RootFragment) shown(screen)).getPreferenceScreen();
    }

    private static void assertAdvancedControls(StreamSettings screen) {
        PreferenceScreen advanced = open(screen, "advanced").getPreferenceScreen();
        assertNull(advanced.findPreference(LEGACY_UNLOCK));
        for (String key : ADVANCED_CONTROLS) assertNotNull(key, advanced.findPreference(key));
    }

    @Test
    public void advancedIsAvailableWithoutAnUnlockAndAboutDoesNotUnlockAnything() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            assertNotNull(root(screen).findPreference("advanced"));
            assertNull(root(screen).findPreference("developer"));
            assertAdvancedControls(screen);
            assertFalse(prefs().contains(LEGACY_UNLOCK));

            Preference about = open(screen, "app").findPreference("about_app");
            about.getOnPreferenceClickListener().onPreferenceClick(about);
            AlertDialog dialog = (AlertDialog) ShadowDialog.getLatestDialog();
            TextView version = dialog.findViewById(android.R.id.message);
            assertTrue(version.getText().toString().contains(com.limelight.BuildConfig.VERSION_NAME));
            for (int tap = 0; tap < 8; tap++) assertFalse(version.performClick());
            dialog.dismiss();

            assertFalse(prefs().contains(LEGACY_UNLOCK));
            assertNull(root(screen).findPreference("developer"));
            assertAdvancedControls(screen);
        }
    }

    @Test
    public void aLegacyUnlockValueDoesNotHideControlsOrRestoreASeparateSection() {
        for (boolean legacy : new boolean[] {false, true}) {
            prefs().edit().putBoolean(LEGACY_UNLOCK, legacy).commit();
            try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
                StreamSettings screen = controller.get();
                assertNull(root(screen).findPreference("developer"));
                assertAdvancedControls(screen);
                assertEquals(legacy, prefs().getBoolean(LEGACY_UNLOCK, !legacy));
            }
        }
    }

    @Test
    public void anOldSectionLinkOpensAdvancedAndSurvivesRecreation() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            assertNotNull(open(screen, "developer").findPreference("checkbox_texture_view"));
            assertEquals(screen.getString(R.string.settings_section_advanced),
                    ((MaterialToolbar) screen.findViewById(R.id.settings_toolbar)).getTitle().toString());
            controller.recreate();
            screen = controller.get();
            settle(screen);
            assertEquals(screen.getString(R.string.settings_section_advanced),
                    ((MaterialToolbar) screen.findViewById(R.id.settings_toolbar)).getTitle().toString());
            assertNotNull(((StreamSettings.SettingsFragment) shown(screen)).findPreference("export_latency_csv"));
        }
    }

    @Test
    public void restoringAnOldSectionAndFragmentShowsOnlyAdvanced() {
        Bundle saved = new Bundle();
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            // Recreate the fragment arguments and activity state written before the section moved.
            screen.getFragmentManager().beginTransaction().replace(R.id.stream_settings,
                    StreamSettings.SettingsFragment.forSection("developer")).commit();
            settle(screen);
            controller.saveInstanceState(saved);
            saved.putString("section", "developer");
        }
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup(saved)) {
            StreamSettings screen = controller.get();
            settle(screen);
            assertEquals(screen.getString(R.string.settings_section_advanced),
                    ((MaterialToolbar) screen.findViewById(R.id.settings_toolbar)).getTitle().toString());
            PreferenceScreen advanced = ((StreamSettings.SettingsFragment) shown(screen)).getPreferenceScreen();
            for (String key : ADVANCED_CONTROLS) assertNotNull(key, advanced.findPreference(key));
            assertNull(advanced.findPreference("list_resolution"));
            assertNull(advanced.findPreference("performance_overlay_mode"));
            assertNull(advanced.findPreference(LEGACY_UNLOCK));
        }
    }

    @Test
    public void advancedContainsTheControlsAndSearchRoutesThere() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            assertAdvancedControls(screen);
            assertNull(open(screen, "advanced").getPreferenceScreen().findPreference("performance_overlay_mode"));
            for (String section : new String[] {"video", "display", "input", "audio", "streaming", "app"}) {
                PreferenceScreen other = open(screen, section).getPreferenceScreen();
                for (String key : ADVANCED_CONTROLS) assertNull(section + ": " + key, other.findPreference(key));
            }
            assertEquals(screen.getString(R.string.settings_section_advanced) + " · " +
                            screen.getString(R.string.settings_category_decoder),
                    resultGroup(search(screen, "compatibility"), "checkbox_texture_view").toString());
        }
    }

    @Test
    public void debugIsAlwaysAnOverlayChoiceOfTheOverlayPicker() {
        assertEquals(Arrays.asList("compact", "statistics", "debug"), Arrays.asList(OverlayChoice.MODES));
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            Preference overlay = open(screen, "streaming").getPreferenceScreen().findPreference("checkbox_enable_perf_overlay");
            assertEquals(screen.getString(R.string.overlay_off), overlay.getSummary().toString());
            prefs().edit().putBoolean("checkbox_enable_perf_overlay", true).putString("performance_overlay_mode", "debug").commit();
            overlay = open(screen, "streaming").getPreferenceScreen().findPreference("checkbox_enable_perf_overlay");
            assertEquals(screen.getString(R.string.overlay_debug), overlay.getSummary().toString());
        }
    }

    @Test
    @Config(shadows = {com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class, AdrenoGpu.class})
    public void movingTheControlsKeepsAllExistingValues() {
        Map<String, Boolean> choices = new LinkedHashMap<>();
        for (String key : ADVANCED_CONTROLS) {
            if (key.startsWith("checkbox_")) choices.put(key, false);
        }
        for (String key : StreamSettings.LOW_LATENCY_CUSTOMIZED) choices.put(key, false);
        choices.put(HEVC, true);
        choices.put("checkbox_texture_view", true);
        choices.put("checkbox_enable_post_stream_toast", true);
        choices.put(GPU_BOOST, true);
        for (boolean legacy : new boolean[] {false, true}) {
            SharedPreferences.Editor editor = prefs().edit().putBoolean(LEGACY_UNLOCK, legacy)
                    .putBoolean("checkbox_unlock_fps", true)
                    // Do not rerun the older migration that enables the latency switches.
                    .putInt(PreferenceConfiguration.PREFS_VERSION_PREF_STRING, PreferenceConfiguration.PROFILE_NATIVE_VERSION)
                    .putString("performance_overlay_mode", "debug")
                    // Choices made after the new engine became the default are kept, off included.
                    .putBoolean(PreferenceConfiguration.RUST_ENGINE_DEFAULT_ON_PREF_STRING, true);
            for (Map.Entry<String, Boolean> entry : choices.entrySet()) editor.putBoolean(entry.getKey(), entry.getValue());
            editor.commit();
            try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
                StreamSettings screen = controller.get();
                for (String section : new String[] {"advanced", "developer"}) {
                    PreferenceScreen advanced = open(screen, section).getPreferenceScreen();
                    openCustomize(advanced);
                    for (Map.Entry<String, Boolean> entry : choices.entrySet()) {
                        boolean shown = HEVC.equals(entry.getKey()) ? !entry.getValue() : entry.getValue();
                        assertEquals(entry.getKey(), shown,
                                ((SwitchPreference) advanced.findPreference(entry.getKey())).isChecked());
                        assertEquals(entry.getKey(), entry.getValue(), prefs().getBoolean(entry.getKey(), !entry.getValue()));
                    }
                }
                PreferenceConfiguration config = PreferenceConfiguration.readPreferences(screen);
                assertTrue(config.useTextureView);
                assertTrue(config.unlockFps);
                assertFalse(config.networkPriority);
                assertTrue(config.gpuMaxClocks);
                assertFalse(config.rustEngine);
                assertEquals("debug", prefs().getString("performance_overlay_mode", null));
            }
        }
    }

    private static int countKey(PreferenceGroup group, String key) {
        int count = 0;
        for (int i = 0; i < group.getPreferenceCount(); i++) {
            Preference item = group.getPreference(i);
            if (key.equals(item.getKey())) count++;
            if (item instanceof PreferenceGroup) count += countKey((PreferenceGroup) item, key);
        }
        return count;
    }

    @Test
    @Config(shadows = {com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class, AdrenoGpu.class})
    public void gpuBoostIsOneOptInAdvancedControlWithTheExistingStoredKey() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            PreferenceScreen advanced = open(screen, "advanced").getPreferenceScreen();
            assertEquals("checkbox_gpu_max_clocks", GPU_BOOST);
            assertEquals(1, countKey(advanced, GPU_BOOST));
            MaterialSwitchPreference boost = (MaterialSwitchPreference) advanced.findPreference(GPU_BOOST);
            assertEquals("Boost GPU performance for PyroWave", boost.getTitle().toString());
            assertEquals("Can reduce decoding time, but uses more power and produces more heat.",
                    boost.getSummary().toString());
            assertNull(boost.getExplanation());
            assertFalse(boost.isChecked());
            assertFalse(PreferenceConfiguration.readPreferences(screen).gpuMaxClocks);

            boost.setChecked(true);
            assertTrue(prefs().getBoolean(GPU_BOOST, false));
            assertEquals(screen.getString(R.string.summary_gpu_max_clocks), boost.getSummary().toString());
            boost = (MaterialSwitchPreference) open(screen, "advanced").findPreference(GPU_BOOST);
            assertTrue(boost.isChecked());
            assertTrue(PreferenceConfiguration.readPreferences(screen).gpuMaxClocks);
            boost.setChecked(false);
            assertFalse(PreferenceConfiguration.readPreferences(screen).gpuMaxClocks);

            for (String section : new String[] {"video", "display", "input", "audio", "streaming", "app"}) {
                assertNull(section, open(screen, section).getPreferenceScreen().findPreference(GPU_BOOST));
            }
            assertEquals(screen.getString(R.string.settings_section_advanced) + " · " +
                            screen.getString(R.string.settings_category_decoder),
                    resultGroup(search(screen, "Boost GPU"), GPU_BOOST).toString());
        }
    }

    @Test
    @Config(shadows = {com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class, NonAdrenoGpu.class})
    public void gpuBoostIsHiddenWithoutAdrenoButDoesNotEraseASavedChoice() {
        prefs().edit().putBoolean(GPU_BOOST, true).commit();
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            assertNull(open(screen, "advanced").findPreference(GPU_BOOST));
            assertNull(resultGroup(search(screen, "Boost GPU"), GPU_BOOST));
            assertTrue(prefs().getBoolean(GPU_BOOST, false));
        }
    }

    @Test
    public void resettingSettingsLeavesAdvancedAndDebugAvailable() {
        prefs().edit().putBoolean(LEGACY_UNLOCK, true).putString("performance_overlay_mode", "debug").commit();
        StreamSettings.resetAllSettings(RuntimeEnvironment.getApplication());
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            assertNull(root(screen).findPreference("developer"));
            assertAdvancedControls(screen);
            Preference overlay = open(screen, "streaming").getPreferenceScreen().findPreference("checkbox_enable_perf_overlay");
            assertEquals(screen.getString(R.string.overlay_off), overlay.getSummary().toString());
            assertEquals(0, OverlayChoice.read(prefs()));
        }
    }
}

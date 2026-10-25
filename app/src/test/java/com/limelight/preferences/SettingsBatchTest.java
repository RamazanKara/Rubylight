package com.limelight.preferences;

import android.app.Application;
import android.content.SharedPreferences;
import android.preference.Preference;
import android.preference.PreferenceManager;
import android.preference.PreferenceScreen;
import android.preference.SwitchPreference;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;

import static com.limelight.preferences.SettingsStructureTest.open;
import static org.junit.Assert.*;

/** The settings simplification batch: what shows, what greys, and which keys the merged controls write. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, qualifiers = "en", application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = {"com.limelight.nvstream.jni", "com.limelight.binding.video"})
public class SettingsBatchTest {
    private static SharedPreferences prefs() {
        return PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication());
    }

    @After
    public void clear() {
        prefs().edit().clear().commit();
    }

    private static PreferenceScreen section(ActivityController<StreamSettings> controller, String section) {
        return open(controller.get(), section).getPreferenceScreen();
    }

    private static android.preference.PreferenceGroup groupOf(PreferenceScreen screen, String key) {
        for (int i = 0; i < screen.getPreferenceCount(); i++) {
            android.preference.PreferenceGroup group = (android.preference.PreferenceGroup) screen.getPreference(i);
            if (group.findPreference(key) != null) return group;
        }
        throw new AssertionError(key);
    }

    // Frame pacing and its neighbours

    @Test
    public void ruleFunctionsFollowTheStoredChoices() {
        SharedPreferences p = prefs();
        p.edit().clear().commit();
        assertTrue(SettingRules.framePacingApplies(p));
        assertFalse(SettingRules.dropLateFramesApplies(p));
        p.edit().putString("frame_pacing", "balanced").commit();
        assertTrue(SettingRules.dropLateFramesApplies(p));
        p.edit().putBoolean("checkbox_host_frame_times", true).commit();
        assertFalse(SettingRules.framePacingApplies(p));
        assertFalse(SettingRules.dropLateFramesApplies(p));
        p.edit().putBoolean("checkbox_host_frame_times", false).putBoolean("checkbox_vrr", true).commit();
        assertFalse(SettingRules.framePacingApplies(p));
        assertFalse(SettingRules.upscalingUsesSharpness(p));
        p.edit().putString("upscaling_mode", "bilinear").commit();
        assertFalse(SettingRules.upscalingUsesSharpness(p));
        p.edit().putString("upscaling_mode", "fsr1").commit();
        assertTrue(SettingRules.upscalingUsesSharpness(p));
        p.edit().putString("upscaling_mode", "sgsr1").commit();
        assertTrue(SettingRules.upscalingUsesSharpness(p));
        assertFalse(SettingRules.qualityLimitsApply(p));
        p.edit().putString("adaptive_quality_mode", "balanced").commit();
        assertTrue(SettingRules.qualityLimitsApply(p));
    }

    @Test
    public void framePacingIsGreyedWhileHostFrameTimesOrAdaptiveRefreshReplaceIt() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            PreferenceScreen display = section(controller, "display");
            Preference pacing = display.findPreference("frame_pacing");
            assertTrue(pacing.isEnabled());
            prefs().edit().putBoolean("checkbox_host_frame_times", true).commit();
            assertFalse(pacing.isEnabled());
            prefs().edit().putBoolean("checkbox_host_frame_times", false).commit();
            assertTrue(pacing.isEnabled());
            prefs().edit().putBoolean("checkbox_vrr", true).commit();
            assertFalse(pacing.isEnabled());
        }
    }

    @Test
    public void frameDroppingSitsBesideFramePacingAndShowsOnlyWithBalanced() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            PreferenceScreen display = section(controller, "display");
            assertNull(display.findPreference("checkbox_drop_late_frames"));
            prefs().edit().putString("frame_pacing", "balanced").commit();
            Preference drop = display.findPreference("checkbox_drop_late_frames");
            assertNotNull(drop);
            Preference pacing = display.findPreference("frame_pacing");
            java.util.List<String> keys = SettingsStructureTest.keys(groupOf(display, "frame_pacing"));
            assertEquals(keys.indexOf("frame_pacing") + 1, keys.indexOf("checkbox_drop_late_frames"));
            assertNotNull(pacing);
            prefs().edit().putString("frame_pacing", "latency").commit();
            assertNull(display.findPreference("checkbox_drop_late_frames"));
            // Hiding it never changes the stored choice, and nothing else shows it.
            assertTrue(prefs().getBoolean("checkbox_drop_late_frames", true));
            assertNull(section(controller, "advanced").findPreference("checkbox_drop_late_frames"));
        }
    }

    @Test
    public void theContentRateVoteNeedsTheLowerRefreshRateSwitch() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            PreferenceScreen display = section(controller, "display");
            Preference vote = display.findPreference("checkbox_content_rate_frame_rate");
            assertEquals("checkbox_reduce_refresh_rate", vote.getDependency());
            assertFalse(vote.isEnabled());
            ((SwitchPreference) display.findPreference("checkbox_reduce_refresh_rate")).setChecked(true);
            assertTrue(vote.isEnabled());
            // Adaptive refresh rate no longer decides it.
            ((SwitchPreference) display.findPreference("checkbox_reduce_refresh_rate")).setChecked(false);
            ((SwitchPreference) display.findPreference("checkbox_vrr")).setChecked(true);
            assertFalse(vote.isEnabled());
        }
    }

    // Frame rate list

    @Test
    public void theFrameRateListOffersAllRatesOnlyWhenUnlockedAndNoSwitchRemainsInAdvanced() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            for (String name : StreamSettings.SECTIONS.keySet()) {
                assertNull(name, section(controller, name).findPreference("checkbox_unlock_fps"));
            }
            assertFalse(rates(controller).contains("120"));
            prefs().edit().putBoolean("checkbox_unlock_fps", true).commit();
            controller.get().reloadSettings();
            SettingsStructureTest.settle(controller.get());
            assertTrue(rates(controller).contains("120"));
            assertTrue(PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication()).unlockFps);
        }
    }

    @Test
    public void theFrameRatePickerButtonTogglesTheStoredUnlockChoice() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            Preference fps = section(controller, "video").findPreference("list_fps");
            assertTrue(fps instanceof FpsPreference);
            ((FpsPreference) fps).showDialog(null);
            androidx.appcompat.app.AlertDialog dialog =
                    (androidx.appcompat.app.AlertDialog) org.robolectric.shadows.ShadowDialog.getLatestDialog();
            android.widget.Button button = dialog.getButton(androidx.appcompat.app.AlertDialog.BUTTON_POSITIVE);
            assertEquals("Show all frame rates", button.getText().toString());
            button.performClick();
            SettingsStructureTest.settle(controller.get());
            assertTrue(prefs().getBoolean("checkbox_unlock_fps", false));
            assertTrue(rates(controller).contains("120"));
            ((FpsPreference) section(controller, "video").findPreference("list_fps")).showDialog(null);
            dialog = (androidx.appcompat.app.AlertDialog) org.robolectric.shadows.ShadowDialog.getLatestDialog();
            assertEquals("Show supported frame rates only",
                    dialog.getButton(androidx.appcompat.app.AlertDialog.BUTTON_POSITIVE).getText().toString());
        }
    }

    private static java.util.List<String> rates(ActivityController<StreamSettings> controller) {
        android.preference.ListPreference fps =
                (android.preference.ListPreference) section(controller, "video").findPreference("list_fps");
        java.util.List<String> values = new java.util.ArrayList<>();
        for (CharSequence value : fps.getEntryValues()) values.add(value.toString());
        return values;
    }

    // Automatic stream quality

    @Test
    public void theQualityModeSitsUnderAutomaticBitrateAndItsLimitsShowOnlyWhenOn() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            PreferenceScreen video = section(controller, "video");
            java.util.List<String> keys = SettingsStructureTest.keys(groupOf(video, "checkbox_auto_bitrate"));
            assertEquals(keys.indexOf("checkbox_auto_bitrate") + 1, keys.indexOf("adaptive_quality_mode"));
            assertEquals("Lower resolution or frame rate when needed", video.findPreference("adaptive_quality_mode").getTitle().toString());
            assertNull(video.findPreference("adaptive_quality_min_resolution"));
            assertNull(video.findPreference("adaptive_quality_min_fps"));
            prefs().edit().putString("adaptive_quality_mode", "balanced").commit();
            keys = SettingsStructureTest.keys(groupOf(video, "checkbox_auto_bitrate"));
            assertEquals(keys.indexOf("adaptive_quality_mode") + 1, keys.indexOf("adaptive_quality_min_resolution"));
            assertEquals(keys.indexOf("adaptive_quality_min_resolution") + 1, keys.indexOf("adaptive_quality_min_fps"));
            prefs().edit().putString("adaptive_quality_mode", "off").commit();
            assertNull(video.findPreference("adaptive_quality_min_fps"));
            // The summary no longer talks about host reconfiguration or an adaptive decoder.
            String summary = RuntimeEnvironment.getApplication().getString(com.limelight.R.string.summary_adaptive_quality_setting);
            assertFalse(summary.contains("reconfiguration"));
            assertFalse(summary.contains("adaptive decoder"));
        }
    }

    // Low-latency decoding

    @Test
    public void customizeFoldsTheIndividualDecodingSwitchesAndSearchStillFindsThem() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            PreferenceScreen advanced = section(controller, "advanced");
            assertNotNull(advanced.findPreference("low_latency_decoding"));
            Preference customize = advanced.findPreference("low_latency_customize");
            assertEquals("Customize", customize.getTitle().toString());
            for (String key : StreamSettings.LOW_LATENCY_CUSTOMIZED) assertNull(key, advanced.findPreference(key));
            // Folded settings are still found by search.
            StreamSettings.SettingsFragment results = SettingsStructureTest.search(controller.get(), "HEVC");
            assertNotNull(SettingsStructureTest.resultGroup(results, "checkbox_disable_hevc_low_latency"));
            SettingsStructureTest.search(controller.get(), "");
            advanced = section(controller, "advanced");
            customize = advanced.findPreference("low_latency_customize");
            customize.getOnPreferenceClickListener().onPreferenceClick(customize);
            java.util.List<String> keys = SettingsStructureTest.keys(groupOf(advanced, "low_latency_customize"));
            assertEquals(keys.indexOf("low_latency_customize") + 1, keys.indexOf("checkbox_codec_low_latency"));
            for (String key : StreamSettings.LOW_LATENCY_CUSTOMIZED) assertTrue(key, keys.contains(key));
            assertEquals("Hide the individual switches", customize.getSummary().toString());
            customize.getOnPreferenceClickListener().onPreferenceClick(customize);
            assertNull(advanced.findPreference("checkbox_codec_performance"));
        }
    }

    @Test
    public void customizeStaysOpenAcrossARotation() {
        android.os.Bundle saved = new android.os.Bundle();
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            PreferenceScreen advanced = section(controller, "advanced");
            Preference customize = advanced.findPreference("low_latency_customize");
            customize.getOnPreferenceClickListener().onPreferenceClick(customize);
            assertNotNull(advanced.findPreference("checkbox_codec_performance"));
            controller.saveInstanceState(saved);
        }
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup(saved)) {
            SettingsStructureTest.settle(controller.get());
            PreferenceScreen advanced = ((StreamSettings.SettingsFragment) SettingsStructureTest.shown(controller.get())).getPreferenceScreen();
            for (String key : StreamSettings.LOW_LATENCY_CUSTOMIZED) assertNotNull(key, advanced.findPreference(key));
            assertEquals("Hide the individual switches", advanced.findPreference("low_latency_customize").getSummary().toString());
        }
    }

    @Test
    public void theHevcSwitchIsWordedPositivelyAndStillStoresTheDisableChoice() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            PreferenceScreen advanced = section(controller, "advanced");
            Preference customize = advanced.findPreference("low_latency_customize");
            customize.getOnPreferenceClickListener().onPreferenceClick(customize);
            SwitchPreference hevc = (SwitchPreference) advanced.findPreference("checkbox_disable_hevc_low_latency");
            assertEquals("HEVC low-latency flags", hevc.getTitle().toString());
            // Nothing stored: the flags are in use, as before, when "disable" was off.
            assertTrue(hevc.isChecked());
            assertFalse(PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication()).disableHevcLowLatency);
            hevc.setChecked(false);
            assertTrue(prefs().getBoolean("checkbox_disable_hevc_low_latency", false));
            assertTrue(PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication()).disableHevcLowLatency);
            hevc.setChecked(true);
            assertFalse(prefs().getBoolean("checkbox_disable_hevc_low_latency", true));
        }
        // An older install that had turned the flags off shows the switch off.
        prefs().edit().putBoolean("checkbox_disable_hevc_low_latency", true).commit();
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            PreferenceScreen advanced = section(controller, "advanced");
            Preference customize = advanced.findPreference("low_latency_customize");
            customize.getOnPreferenceClickListener().onPreferenceClick(customize);
            assertFalse(((SwitchPreference) advanced.findPreference("checkbox_disable_hevc_low_latency")).isChecked());
        }
    }

    // Image

    @Test
    public void sharpnessShowsOnlyForTheSharpUpscalers() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            PreferenceScreen display = section(controller, "display");
            assertNull(display.findPreference("upscaling_sharpness"));
            prefs().edit().putString("upscaling_mode", "bilinear").commit();
            assertNull(display.findPreference("upscaling_sharpness"));
            prefs().edit().putString("upscaling_mode", "fsr1").commit();
            assertNotNull(display.findPreference("upscaling_sharpness"));
            prefs().edit().putString("upscaling_mode", "sgsr1").commit();
            assertNotNull(display.findPreference("upscaling_sharpness"));
            prefs().edit().putString("upscaling_mode", "off").commit();
            assertNull(display.findPreference("upscaling_sharpness"));
        }
    }

    @Test
    public void theUpscalingEntriesUseThePlainNamesAndTheValuesStayTheSame() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            android.preference.ListPreference mode =
                    (android.preference.ListPreference) section(controller, "display").findPreference("upscaling_mode");
            java.util.List<String> names = new java.util.ArrayList<>();
            for (CharSequence name : mode.getEntries()) names.add(name.toString());
            java.util.List<String> values = new java.util.ArrayList<>();
            for (CharSequence value : mode.getEntryValues()) values.add(value.toString());
            assertEquals(java.util.Arrays.asList("Off", "Smooth", "Sharp (FSR)", "Sharp, Snapdragon (SGSR)"), names);
            assertEquals(java.util.Arrays.asList("off", "bilinear", "fsr1", "sgsr1"), values);
        }
    }

    @Test
    public void vulkanRenderingAndFullRangeMoveToAdvancedAndKeepTheirValues() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            PreferenceScreen display = section(controller, "display");
            PreferenceScreen advanced = section(controller, "advanced");
            for (String key : new String[] {"checkbox_vulkan_render", "checkbox_full_range"}) {
                assertNull(key, display.findPreference(key));
                assertNotNull(key, advanced.findPreference(key));
                assertEquals("category_advanced_decoder", groupOf(advanced, key).getKey());
            }
        }
    }

    // Optimize game settings and render scale

    @Test
    public void gameOptimizationHintIsUnderCompatibilityAndStaysOnByDefault() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            PreferenceScreen advanced = section(controller, "advanced");
            SwitchPreference sops = (SwitchPreference) advanced.findPreference("checkbox_enable_sops");
            assertEquals("category_advanced_compatibility", groupOf(advanced, "checkbox_enable_sops").getKey());
            assertEquals("Game optimization hint", sops.getTitle().toString());
            assertTrue(sops.isChecked());
            assertEquals("Lets the PC adjust the game’s settings", sops.getSummary().toString());
            // User-facing text stays host-neutral: no host brand and no host-specific variable name.
            String explanation = ((MaterialSwitchPreference) sops).getExplanation().toString();
            assertFalse(explanation.contains("SUNSHINE_CLIENT_ENABLE_SOPS"));
            assertFalse(explanation.contains("Rubylight"));
            assertNull(section(controller, "video").findPreference("checkbox_enable_sops"));
        }
    }

    @Test
    public void theHostRenderScaleIsNamedAfterThePcRenderResolution() {
        assertEquals("PC render resolution",
                RuntimeEnvironment.getApplication().getString(com.limelight.R.string.title_virtual_display_scale));
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            Preference scale = section(controller, "display").findPreference("seekbar_virtual_display_scale");
            assertEquals("PC render resolution", scale.getTitle().toString());
        }
    }

    // Pickers that write the two older switches

    @Test
    public void themeChoiceWritesBothOlderSwitchesAndReadsOlderValues() {
        SharedPreferences p = prefs();
        assertEquals(ThemeChoice.SYSTEM, ThemeChoice.read(p));
        p.edit().putBoolean("checkbox_always_dark", true).commit();
        assertEquals(ThemeChoice.DARK, ThemeChoice.read(p));
        p.edit().putBoolean("checkbox_oled_black", true).commit();
        assertEquals(ThemeChoice.OLED_BLACK, ThemeChoice.read(p));
        // OLED black without Dark was a value the older two switches could hold.
        p.edit().putBoolean("checkbox_always_dark", false).commit();
        assertEquals(ThemeChoice.OLED_BLACK, ThemeChoice.read(p));
        for (int choice = 0; choice < 3; choice++) {
            SharedPreferences.Editor editor = p.edit();
            ThemeChoice.save(editor, choice);
            editor.commit();
            assertEquals(choice, ThemeChoice.read(p));
            assertEquals(choice != 0, p.getBoolean("checkbox_always_dark", false));
            assertEquals(choice == 2, p.getBoolean("checkbox_oled_black", false));
        }
    }

    @Test
    public void overlayChoiceWritesTheSwitchAndTheModeAndReadsOlderValues() {
        SharedPreferences p = prefs();
        assertEquals(0, OverlayChoice.read(p));
        p.edit().putBoolean("checkbox_enable_perf_overlay", true).commit();
        assertEquals("Switch on, no mode stored: the default Compact", 1, OverlayChoice.read(p));
        p.edit().putString("performance_overlay_mode", "statistics").commit();
        assertEquals(2, OverlayChoice.read(p));
        p.edit().putString("performance_overlay_mode", "debug").commit();
        assertEquals(3, OverlayChoice.read(p));
        p.edit().putString("performance_overlay_mode", "bogus").commit();
        assertEquals(1, OverlayChoice.read(p));
        p.edit().putBoolean("checkbox_enable_perf_overlay", false).putString("performance_overlay_mode", "debug").commit();
        assertEquals("A mode kept while the switch is off still reads as Off", 0, OverlayChoice.read(p));
        for (int choice = 0; choice < 4; choice++) {
            SharedPreferences.Editor editor = p.edit();
            OverlayChoice.save(editor, choice);
            editor.commit();
            assertEquals(choice, OverlayChoice.read(p));
            assertEquals(choice != 0, p.getBoolean("checkbox_enable_perf_overlay", false));
            if (choice != 0) assertEquals(OverlayChoice.MODES[choice - 1], p.getString("performance_overlay_mode", ""));
        }
        SharedPreferences.Editor editor = p.edit();
        OverlayChoice.save(editor, 0);
        editor.commit();
        assertEquals("Off keeps the last mode for the next time", "debug", p.getString("performance_overlay_mode", ""));
    }

    @Test
    public void highContrastIsNotedOnTheThemePickerBecauseItKeepsTheAppBlack() {
        prefs().edit().putBoolean("checkbox_high_contrast", true).commit();
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            Preference theme = section(controller, "app").findPreference("checkbox_always_dark");
            assertEquals("System\nHigh contrast keeps the app black", theme.getSummary().toString());
        }
    }

    @Test
    public void thePickersOnTheInterfaceAndStreamPagesWriteTheStoredKeys() {
        prefs().edit().putBoolean("checkbox_always_dark", true).commit();
        // Choosing a theme restarts the screen, so these controllers are left open and each choice is read back
        // from a fresh screen.
        ActivityController<StreamSettings> first = Robolectric.buildActivity(StreamSettings.class).setup();
        PreferenceScreen app = section(first, "app");
        Preference theme = app.findPreference("checkbox_always_dark");
        assertNull(app.findPreference("checkbox_oled_black"));
        assertEquals("Theme", theme.getTitle().toString());
        assertEquals("Dark", theme.getSummary().toString());
        choose(theme, 2);
        assertTrue(prefs().getBoolean("checkbox_always_dark", false));
        assertTrue(prefs().getBoolean("checkbox_oled_black", false));
        ActivityController<StreamSettings> second = Robolectric.buildActivity(StreamSettings.class).setup();
        theme = section(second, "app").findPreference("checkbox_always_dark");
        assertEquals("OLED black", theme.getSummary().toString());
        choose(theme, 0);
        assertFalse(prefs().getBoolean("checkbox_always_dark", true));
        assertFalse(prefs().getBoolean("checkbox_oled_black", true));
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            assertEquals("System", section(controller, "app").findPreference("checkbox_always_dark").getSummary().toString());
            PreferenceScreen streaming = section(controller, "streaming");
            Preference overlay = streaming.findPreference("checkbox_enable_perf_overlay");
            assertNull(streaming.findPreference("performance_overlay_mode"));
            assertEquals("Performance overlay", overlay.getTitle().toString());
            assertEquals("Off", overlay.getSummary().toString());
            choose(overlay, 2);
            assertTrue(prefs().getBoolean("checkbox_enable_perf_overlay", false));
            assertEquals("statistics", prefs().getString("performance_overlay_mode", ""));
            assertEquals("Statistics", overlay.getSummary().toString());
            choose(overlay, 0);
            assertFalse(prefs().getBoolean("checkbox_enable_perf_overlay", true));
            assertEquals("Off", overlay.getSummary().toString());
        }
    }

    /** Opens a picker row and taps the choice at {@code index}. */
    private static void choose(Preference row, int index) {
        row.getOnPreferenceClickListener().onPreferenceClick(row);
        androidx.appcompat.app.AlertDialog dialog =
                (androidx.appcompat.app.AlertDialog) org.robolectric.shadows.ShadowDialog.getLatestDialog();
        android.widget.ListView list = dialog.getListView();
        list.performItemClick(list.getChildAt(index), index, list.getAdapter().getItemId(index));
        assertFalse(dialog.isShowing());
    }
}

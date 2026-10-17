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
import com.limelight.HelpTopicsActivity;
import com.limelight.R;

import org.junit.After;
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

    static StreamSettings.SettingsFragment open(StreamSettings screen, String section) {
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

    @After
    public void clear() {
        prefs().edit().clear().commit();
    }

    @Test
    public void overviewListsTheSectionsInOrder() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            settle(screen);
            PreferenceScreen root = ((StreamSettings.RootFragment) shown(screen)).getPreferenceScreen();
            assertEquals(Arrays.asList("stream_presets", "video", "display", "input", "audio", "streaming", "app", "advanced"),
                    keys(root));
            assertEquals(screen.getString(R.string.settings_section_display), root.findPreference("display").getTitle());
            assertNotNull(root.findPreference("display").getSummary());
            assertNotNull(root.findPreference("audio").getSummary());
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
            assertEquals(Arrays.asList("category_gamepad_settings", "category_usb_controllers", "category_input_settings",
                    "category_onscreen_controls", "category_keyboard_shortcuts"), keys(controls));
            assertEquals(Arrays.asList("checkbox_usb_driver", "checkbox_usb_bind_all", "checkbox_usb_dualsense",
                    "checkbox_usb_dualsense_haptics", "checkbox_usb_switch_pro"), keys((PreferenceGroup) controls.getPreference(1)));
            PreferenceGroup image = (PreferenceGroup) open(screen, "display").getPreferenceScreen().findPreference("category_upscaling");
            assertTrue(keys(image).containsAll(Arrays.asList("checkbox_yuv444", "checkbox_full_range")));
            PreferenceScreen advanced = open(screen, "advanced").getPreferenceScreen();
            assertEquals(Arrays.asList(StreamSettings.LOW_LATENCY_DECODING_KEY, "checkbox_enable_sops"),
                    keys((PreferenceGroup) advanced.getPreference(0)));
        }
    }

    @Test
    public void searchNamesTheSectionOnceAndTheGroupWhenThereIsOne() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            StreamSettings.SettingsFragment results = search(screen, "language");
            assertEquals(screen.getString(R.string.settings_section_app), resultGroup(results, "list_languages").toString());
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
            StreamSettings.SettingsFragment video = open(controller.get(), "video");
            ListPreference mode = (ListPreference) video.findPreference("adaptive_quality_mode");
            Preference resolution = video.findPreference("adaptive_quality_min_resolution");
            Preference fps = video.findPreference("adaptive_quality_min_fps");
            assertEquals("off", mode.getValue());
            assertFalse(resolution.isEnabled());
            assertFalse(fps.isEnabled());
            assertTrue(mode.getOnPreferenceChangeListener().onPreferenceChange(mode, "motion"));
            mode.setValue("motion");
            assertTrue(resolution.isEnabled());
            assertTrue(fps.isEnabled());
            assertTrue(mode.getOnPreferenceChangeListener().onPreferenceChange(mode, "off"));
            mode.setValue("off");
            assertFalse(resolution.isEnabled());
            assertFalse(fps.isEnabled());
        }
        // A stored mode enables them when the screen opens.
        prefs().edit().putString("adaptive_quality_mode", "detail").commit();
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            assertTrue(open(controller.get(), "video").findPreference("adaptive_quality_min_fps").isEnabled());
        }
    }

    @Test
    public void adaptiveRefreshDisablesFramePacingAndTheLowerRefreshRate() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings.SettingsFragment display = open(controller.get(), "display");
            SwitchPreference vrr = (SwitchPreference) display.findPreference("checkbox_vrr");
            assertTrue(display.findPreference("frame_pacing").isEnabled());
            vrr.setChecked(true);
            assertFalse(display.findPreference("frame_pacing").isEnabled());
            assertFalse(display.findPreference("checkbox_reduce_refresh_rate").isEnabled());
            vrr.setChecked(false);
            assertTrue(display.findPreference("checkbox_reduce_refresh_rate").isEnabled());
        }
    }

    @Test
    public void lowLatencyDecodingSetsItsPartsAndSaysWhenOneIsOff() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            StreamSettings.SettingsFragment advanced = open(screen, "advanced");
            SwitchPreference merged = (SwitchPreference) advanced.findPreference(StreamSettings.LOW_LATENCY_DECODING_KEY);
            assertTrue(merged.isChecked());
            assertNull(merged.getSummary());

            prefs().edit().putBoolean("checkbox_vendor_low_latency", false).commit();
            assertFalse(merged.isChecked());
            assertEquals(screen.getString(R.string.settings_low_latency_partial), merged.getSummary().toString());

            assertTrue(merged.getOnPreferenceChangeListener().onPreferenceChange(merged, true));
            for (String key : PreferenceConfiguration.LOW_LATENCY_DECODING_KEYS) assertTrue(key, prefs().getBoolean(key, false));
            assertTrue(merged.isChecked());
            assertNull(merged.getSummary());

            assertTrue(merged.getOnPreferenceChangeListener().onPreferenceChange(merged, false));
            for (String key : PreferenceConfiguration.LOW_LATENCY_DECODING_KEYS) assertFalse(key, prefs().getBoolean(key, true));
            assertFalse(merged.isChecked());
            assertNull(merged.getSummary());
            // Nothing is stored under the merged switch's own key.
            assertFalse(prefs().contains(StreamSettings.LOW_LATENCY_DECODING_KEY));
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
            SwitchPreference color = (SwitchPreference) display.findPreference("checkbox_yuv444");
            View row = color.getView(null, parent);
            assertEquals(View.GONE, row.findViewById(android.R.id.summary).getVisibility());
            assertEquals(View.VISIBLE, row.findViewById(R.id.preference_info).getVisibility());
            int enabledTitle = ((TextView) row.findViewById(android.R.id.title)).getCurrentTextColor();

            ((SwitchPreference) display.findPreference("checkbox_vrr")).setChecked(true);
            Preference lower = display.findPreference("checkbox_reduce_refresh_rate");
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

            // A switch without an explanation has no info button, and a host-dependent one says so briefly.
            TextView hostLine = display.findPreference("checkbox_virtual_display").getView(null, parent)
                    .findViewById(android.R.id.summary);
            assertEquals(View.VISIBLE, hostLine.getVisibility());
            assertEquals(screen.getString(R.string.settings_needs_host_support), hostLine.getText().toString());
            View guide = open(screen, "input").findPreference("checkbox_show_guide_button").getView(null, parent);
            assertEquals(View.GONE, guide.findViewById(R.id.preference_info).getVisibility());
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
            assertEquals(MaterialColors.getColor(confirm.getContext(), androidx.appcompat.R.attr.colorError, 0),
                    confirm.getButton(AlertDialog.BUTTON_POSITIVE).getCurrentTextColor());
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

package com.limelight.preferences;

import android.app.Application;
import android.content.Intent;
import android.content.SharedPreferences;
import androidx.appcompat.app.AlertDialog;
import com.limelight.R;
import com.limelight.ui.UiNavigationTest;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.util.ReflectionHelpers;
import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class SettingsInteractionTest {
    @Test
    public void untouchedOverlaysBecomeCompactOnceAndChoicesSurvive() {
        Application app = RuntimeEnvironment.getApplication();
        SharedPreferences prefs = android.preference.PreferenceManager.getDefaultSharedPreferences(app);
        String[][] upgrades = {
                // Stored before compact was the default: what it becomes on the first read.
                {null, "compact"}, {"statistics", "compact"}, {"compact", "compact"},
                {"expanded", "statistics"}, {"advanced", "debug"}, {"debug", "debug"}};
        for (String[] upgrade : upgrades) {
            SharedPreferences.Editor editor = prefs.edit().clear();
            if (upgrade[0] != null) editor.putString("performance_overlay_mode", upgrade[0]);
            editor.commit();
            PreferenceConfiguration.readPreferences(app);
            assertEquals(upgrade[0], upgrade[1], prefs.getString("performance_overlay_mode", null));
            // A later choice of Statistics, in settings or the stream menu, stays.
            prefs.edit().putString("performance_overlay_mode", "statistics").commit();
            PreferenceConfiguration.readPreferences(app);
            assertEquals("statistics", prefs.getString("performance_overlay_mode", null));
        }
    }

    @Test
    public void aNewInstallDefaultsToTheCompactOverlay() {
        Application app = RuntimeEnvironment.getApplication();
        SharedPreferences prefs = android.preference.PreferenceManager.getDefaultSharedPreferences(app);
        prefs.edit().clear().commit();
        android.preference.PreferenceManager.setDefaultValues(app, R.xml.preferences, true);
        // The overlay picker has no default of its own; nothing is stored until it is used, and that reads as Off.
        assertEquals(0, OverlayChoice.read(prefs));
        PreferenceConfiguration.readPreferences(app);
        assertEquals("compact", prefs.getString("performance_overlay_mode", "compact"));
    }

    @Test
    public void explicitDisplayModeLeadsTheDisplayGroupAndControlsVirtualDisplay() {
        Application app = RuntimeEnvironment.getApplication();
        HostDisplayChoice.save(app, "exclusive-host", HostDisplayChoice.EXCLUSIVE);
        Intent intent = new Intent(app, HostStreamSettings.class)
                .putExtra("host_uuid", "exclusive-host").putExtra("host_name", "PC");
        ActivityController<HostStreamSettings> controller = Robolectric.buildActivity(HostStreamSettings.class, intent).setup();
        try {
            HostStreamSettings.SettingsFragment fragment = (HostStreamSettings.SettingsFragment)
                    controller.get().getFragmentManager().findFragmentById(R.id.host_profile_settings);
            // The PC's own stream values come first; the display on the PC follows as its own group.
            android.preference.PreferenceGroup display = (android.preference.PreferenceGroup)
                    fragment.getPreferenceScreen().getPreference(1);
            assertEquals(app.getString(R.string.pc_settings_display), display.getTitle().toString());
            assertEquals("host_display", display.getPreference(0).getKey());
            assertTrue(fragment.findPreference("host_display").getSummary().toString()
                    .startsWith(app.getString(R.string.host_display_exclusive)));
            assertFalse(fragment.findPreference("virtual").isEnabled());
            assertTrue(fragment.findPreference("virtual").getSummary().toString()
                    .contains(app.getString(R.string.host_display_exclusive)));
            HostDisplayChoice.save(app, "exclusive-host", HostDisplayChoice.HOST_DEFAULT);
            ReflectionHelpers.callInstanceMethod(fragment, "updateSummaries");
            assertTrue(fragment.findPreference("virtual").isEnabled());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void backOffersToSaveOrDiscardAChangedHostProfile() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), HostStreamSettings.class)
                .putExtra("host_uuid", "profile-host").putExtra("host_name", "PC");
        ActivityController<HostStreamSettings> controller = Robolectric.buildActivity(HostStreamSettings.class, intent).setup();
        try {
            HostStreamSettings screen = controller.get();
            HostStreamProfile draft = ReflectionHelpers.getField(screen, "overrides");
            draft.bitrate = 45500;
            screen.getOnBackPressedDispatcher().onBackPressed();
            AlertDialog dialog = (AlertDialog) ShadowDialog.getLatestDialog();
            assertTrue(dialog.isShowing());
            assertFalse(screen.isFinishing());
            assertNull(HostStreamProfile.load(screen, "profile-host"));
            dialog.getButton(AlertDialog.BUTTON_NEUTRAL).performClick();
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertFalse(screen.isFinishing());
            screen.getOnBackPressedDispatcher().onBackPressed();
            ((AlertDialog) ShadowDialog.getLatestDialog()).getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertTrue(screen.isFinishing());
            assertEquals(Integer.valueOf(45500), HostStreamProfile.load(screen, "profile-host").bitrate);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void discardingAProfileKeepsItsSavedValues() {
        Application app = RuntimeEnvironment.getApplication();
        HostStreamProfile original = new HostStreamProfile();
        original.bitrate = 30000;
        original.save(app, "discard-host");
        Intent intent = new Intent(app, HostStreamSettings.class).putExtra("host_uuid", "discard-host");
        ActivityController<HostStreamSettings> controller = Robolectric.buildActivity(HostStreamSettings.class, intent).setup();
        try {
            HostStreamProfile draft = ReflectionHelpers.getField(controller.get(), "overrides");
            draft.bitrate = 50000;
            controller.get().getOnBackPressedDispatcher().onBackPressed();
            ((AlertDialog) ShadowDialog.getLatestDialog()).getButton(AlertDialog.BUTTON_NEGATIVE).performClick();
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertTrue(controller.get().isFinishing());
            assertEquals(Integer.valueOf(30000), HostStreamProfile.load(app, "discard-host").bitrate);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void displayHelpKeepsTheOriginalPickerOpenAndDoesNotSave() {
        ActivityController<UiNavigationTest.Screen> controller = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup();
        try {
            HostDisplaySettings.show(controller.get(), "display-host", "PC");
            AlertDialog picker = (AlertDialog) ShadowDialog.getLatestDialog();
            picker.getButton(AlertDialog.BUTTON_NEUTRAL).performClick();
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            AlertDialog help = (AlertDialog) ShadowDialog.getLatestDialog();
            assertNotSame(picker, help);
            assertTrue(picker.isShowing());
            help.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertTrue(picker.isShowing());
            assertEquals(HostDisplayChoice.HOST_DEFAULT, HostDisplayChoice.load(controller.get(), "display-host"));
            picker.dismiss();
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void resolutionAndFrameRateChangesMoveADefaultBitrateButKeepAChosenOne() {
        Application app = RuntimeEnvironment.getApplication();
        SharedPreferences prefs = android.preference.PreferenceManager.getDefaultSharedPreferences(app);
        // An install upgraded by an earlier version: its native-defaults migration stored the
        // default bitrate, with no record of whether the user ever chose one.
        prefs.edit().clear().putString(PreferenceConfiguration.RESOLUTION_PREF_STRING, "1280x720")
                .putString(PreferenceConfiguration.FPS_PREF_STRING, "60")
                .putInt(PreferenceConfiguration.BITRATE_PREF_STRING, PreferenceConfiguration.getDefaultBitrate("1280x720", "60"))
                .putInt(PreferenceConfiguration.PREFS_VERSION_PREF_STRING, PreferenceConfiguration.PROFILE_NATIVE_VERSION)
                .commit();
        ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup();
        try {
            StreamSettings screen = controller.get();
            screen.showSection("video");
            org.robolectric.shadows.ShadowLooper.idleMainLooper(150, java.util.concurrent.TimeUnit.MILLISECONDS);
            screen.getFragmentManager().executePendingTransactions();
            StreamSettings.SettingsFragment fragment = (StreamSettings.SettingsFragment)
                    screen.getFragmentManager().findFragmentById(R.id.stream_settings);

            choose(fragment, PreferenceConfiguration.RESOLUTION_PREF_STRING, "1920x1080");
            assertEquals(PreferenceConfiguration.getDefaultBitrate("1920x1080", "60"),
                    prefs.getInt(PreferenceConfiguration.BITRATE_PREF_STRING, 0));
            choose(fragment, PreferenceConfiguration.FPS_PREF_STRING, "120");
            assertEquals(PreferenceConfiguration.getDefaultBitrate("1920x1080", "120"),
                    prefs.getInt(PreferenceConfiguration.BITRATE_PREF_STRING, 0));

            // A bitrate the user picks stays, even when it equals the current size's default.
            for (int chosen : new int[] {45500, PreferenceConfiguration.getDefaultBitrate("1920x1080", "120")}) {
                android.preference.Preference bitrate = fragment.findPreference(PreferenceConfiguration.BITRATE_PREF_STRING);
                assertTrue(bitrate.getOnPreferenceChangeListener().onPreferenceChange(bitrate, chosen));
                prefs.edit().putInt(PreferenceConfiguration.BITRATE_PREF_STRING, chosen).commit();
                choose(fragment, PreferenceConfiguration.RESOLUTION_PREF_STRING, "1280x720");
                choose(fragment, PreferenceConfiguration.FPS_PREF_STRING, "60");
                assertEquals(chosen, prefs.getInt(PreferenceConfiguration.BITRATE_PREF_STRING, 0));
                choose(fragment, PreferenceConfiguration.RESOLUTION_PREF_STRING, "1920x1080");
                choose(fragment, PreferenceConfiguration.FPS_PREF_STRING, "120");
            }
        } finally {
            controller.pause().stop().destroy();
            prefs.edit().clear().commit();
        }
    }

    @Test
    public void olderInstallsKeepACustomBitrateAndFollowTheDefaultOne() {
        SharedPreferences prefs = RuntimeEnvironment.getApplication().getSharedPreferences("bitrate-test", 0);
        for (int stored : new int[] {45500, PreferenceConfiguration.getDefaultBitrate("1280x720", "60")}) {
            prefs.edit().clear().putString(PreferenceConfiguration.RESOLUTION_PREF_STRING, "1280x720")
                    .putString(PreferenceConfiguration.FPS_PREF_STRING, "60")
                    .putInt(PreferenceConfiguration.BITRATE_PREF_STRING, stored).commit();
            assertEquals(stored == 45500, PreferenceConfiguration.isBitrateUserSet(prefs));
        }
        prefs.edit().clear().commit();
        assertFalse(PreferenceConfiguration.isBitrateUserSet(prefs));
    }

    @Test
    public void aStreamAtTheScreensSizeKeepsAChosenBitrateAndMovesADefaultOne() {
        Application app = RuntimeEnvironment.getApplication();
        SharedPreferences prefs = android.preference.PreferenceManager.getDefaultSharedPreferences(app);
        String screen = PreferenceConfiguration.nativeResolution(app);
        assertNotNull(screen);
        assertNotEquals("1920x1080", screen);
        int savedDefault = PreferenceConfiguration.getDefaultBitrate("1920x1080", "60");
        int screenDefault = PreferenceConfiguration.getDefaultBitrate(screen, "60");
        assertNotEquals(savedDefault, screenDefault);
        // {stored bitrate, user-set flag (null: an install from before the flag), bitrate streamed}
        Object[][] cases = {
                // Picked by the user, even when it equals the saved size's default.
                {savedDefault, true, savedDefault},
                {45500, true, 45500},
                // Written by a preset, the migration or a resolution change: it follows the size in use.
                {45500, false, screenDefault},
                {savedDefault, false, screenDefault},
                // Older installs keep the heuristic: only the saved size's default follows.
                {savedDefault, null, screenDefault},
                {45500, null, 45500},
        };
        try {
            for (Object[] c : cases) {
                SharedPreferences.Editor editor = prefs.edit().clear()
                        .putInt(PreferenceConfiguration.PREFS_VERSION_PREF_STRING, PreferenceConfiguration.PROFILE_NATIVE_VERSION)
                        .putString(PreferenceConfiguration.RESOLUTION_PREF_STRING, "1920x1080")
                        .putString(PreferenceConfiguration.FPS_PREF_STRING, "60")
                        .putBoolean(PreferenceConfiguration.RESOLUTION_FOLLOWS_SCREEN_PREF_STRING, true)
                        .putInt(PreferenceConfiguration.BITRATE_PREF_STRING, (Integer) c[0]);
                if (c[1] != null) editor.putBoolean(PreferenceConfiguration.BITRATE_USER_SET_PREF_STRING, (Boolean) c[1]);
                editor.commit();
                PreferenceConfiguration config = PreferenceConfiguration.readPreferences(app);
                assertEquals(screen, PreferenceConfiguration.getResolutionText(config.width, config.height));
                assertEquals(c[0] + ", user-set " + c[1], (int) (Integer) c[2], config.bitrate);
            }
        } finally {
            prefs.edit().clear().commit();
        }
    }

    private static void choose(StreamSettings.SettingsFragment fragment, String key, String value) {
        android.preference.ListPreference list = (android.preference.ListPreference) fragment.findPreference(key);
        // What ListPreference does when the user picks an entry.
        if (list.getOnPreferenceChangeListener().onPreferenceChange(list, value)) list.setValue(value);
    }

    @Test
    public void presetRowCanBeActivatedWithoutTouchAndRequiresConfirmation() {
        ActivityController<UiNavigationTest.Screen> controller = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup();
        try {
            PresetPreference preference = new PresetPreference(controller.get(), null);
            assertTrue(preference.isSelectable());
            ReflectionHelpers.callInstanceMethod(preference, "onClick");
            AlertDialog chooser = (AlertDialog) ShadowDialog.getLatestDialog();
            assertTrue(chooser.isShowing());
            chooser.getListView().performItemClick(null, 0, 0);
            AlertDialog confirmation = (AlertDialog) ShadowDialog.getLatestDialog();
            assertNotSame(chooser, confirmation);
            assertEquals(controller.get().getString(R.string.preset_replace_settings),
                    ((android.widget.TextView) confirmation.findViewById(android.R.id.message)).getText().toString());
            confirmation.getButton(AlertDialog.BUTTON_NEGATIVE).performClick();
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void listHelpPreservesTheOpenPickerAndItsSelection() {
        ActivityController<UiNavigationTest.Screen> controller = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup();
        try {
            MaterialListPreference preference = new MaterialListPreference(controller.get(), null);
            preference.setTitle("Video format");
            preference.setDialogMessage("Choose the format supported by your PC.");
            preference.setEntries(new String[] {"Automatic", "HEVC"});
            preference.setEntryValues(new String[] {"auto", "hevc"});
            preference.setValue("hevc");
            preference.showDialog(null);
            AlertDialog picker = (AlertDialog) ShadowDialog.getLatestDialog();
            assertEquals(1, picker.getListView().getCheckedItemPosition());
            picker.getButton(AlertDialog.BUTTON_NEUTRAL).performClick();
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            AlertDialog help = (AlertDialog) ShadowDialog.getLatestDialog();
            assertNotSame(picker, help);
            assertTrue(picker.isShowing());
            help.dismiss();
            assertEquals("hevc", preference.getValue());
            picker.dismiss();
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void extendedDisplayChoicesSaveTheExactLabeledSemantics() {
        ActivityController<UiNavigationTest.Screen> controller = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup();
        try {
            for (HostDisplayChoice choice : new HostDisplayChoice[] {
                    HostDisplayChoice.EXTENDED_PRIMARY, HostDisplayChoice.EXTENDED,
                    HostDisplayChoice.EXTENDED_ISOLATED, HostDisplayChoice.EXTENDED_PRIMARY_ISOLATED}) {
                HostDisplayChoice.save(controller.get(), "picker-semantics", HostDisplayChoice.HOST_DEFAULT);
                HostDisplaySettings.show(controller.get(), "picker-semantics", "PC");
                AlertDialog picker = (AlertDialog) ShadowDialog.getLatestDialog();
                int index = displayChoiceIndex(picker, controller.get().getString(choice.label));
                assertTrue(displayChoiceIndex(picker, controller.get().getString(HostDisplayChoice.EXTENDED.label)) <
                        displayChoiceIndex(picker, controller.get().getString(HostDisplayChoice.EXTENDED_PRIMARY.label)));
                picker.getListView().performItemClick(null, index, index);
                assertEquals(HostDisplayChoice.HOST_DEFAULT, HostDisplayChoice.load(controller.get(), "picker-semantics"));
                assertTrue(picker.isShowing());
                picker.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
                org.robolectric.shadows.ShadowLooper.idleMainLooper();
                assertEquals(choice, HostDisplayChoice.load(controller.get(), "picker-semantics"));
            }
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void cancelingDisplayModeKeepsSavedChoiceAndDoesNotNotifyChange() {
        ActivityController<UiNavigationTest.Screen> controller = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup();
        try {
            HostDisplayChoice.save(controller.get(), "cancel-display", HostDisplayChoice.EXTENDED);
            java.util.concurrent.atomic.AtomicInteger changes = new java.util.concurrent.atomic.AtomicInteger();
            HostDisplaySettings.show(controller.get(), "cancel-display", "PC", changes::incrementAndGet);
            AlertDialog picker = (AlertDialog) ShadowDialog.getLatestDialog();
            int index = displayChoiceIndex(picker, controller.get().getString(HostDisplayChoice.EXTENDED_PRIMARY.label));
            picker.getListView().performItemClick(null, index, index);
            picker.getButton(AlertDialog.BUTTON_NEGATIVE).performClick();
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertEquals(HostDisplayChoice.EXTENDED, HostDisplayChoice.load(controller.get(), "cancel-display"));
            assertEquals(0, changes.get());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    private static int displayChoiceIndex(AlertDialog picker, String label) {
        for (int i = 0; i < picker.getListView().getCount(); i++) {
            if (label.equals(picker.getListView().getAdapter().getItem(i))) return i;
        }
        throw new AssertionError("Missing display choice: " + label);
    }

    @Test
    public void unconfirmedDisplayChoiceSurvivesRecreationAndOnlySavePersistsIt() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), HostStreamSettings.class)
                .putExtra("host_uuid", "display-draft-host").putExtra("host_name", "PC");
        ActivityController<HostStreamSettings> controller = Robolectric.buildActivity(HostStreamSettings.class, intent).setup();
        try {
            for (boolean save : new boolean[] {false, true}) {
                HostStreamSettings.SettingsFragment fragment = (HostStreamSettings.SettingsFragment)
                        controller.get().getFragmentManager().findFragmentById(R.id.host_profile_settings);
                fragment.findPreference("host_display").getOnPreferenceClickListener()
                        .onPreferenceClick(fragment.findPreference("host_display"));
                AlertDialog picker = (AlertDialog) ShadowDialog.getLatestDialog();
                String label = controller.get().getString(HostDisplayChoice.EXTENDED_PRIMARY.label);
                int index = displayChoiceIndex(picker, label);
                picker.getListView().performItemClick(null, index, index);
                assertEquals(HostDisplayChoice.HOST_DEFAULT, HostDisplayChoice.load(controller.get(), "display-draft-host"));
                controller.recreate();
                picker = (AlertDialog) ShadowDialog.getLatestDialog();
                assertTrue(picker.isShowing());
                assertEquals(label, picker.getListView().getItemAtPosition(picker.getListView().getCheckedItemPosition()));
                assertEquals(HostDisplayChoice.HOST_DEFAULT, HostDisplayChoice.load(controller.get(), "display-draft-host"));
                picker.getButton(save ? AlertDialog.BUTTON_POSITIVE : AlertDialog.BUTTON_NEGATIVE).performClick();
                org.robolectric.shadows.ShadowLooper.idleMainLooper();
                assertEquals(save ? HostDisplayChoice.EXTENDED_PRIMARY : HostDisplayChoice.HOST_DEFAULT,
                        HostDisplayChoice.load(controller.get(), "display-draft-host"));
            }
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void unconfirmedNumericEditorSurvivesRecreationAndDoesNotSaveEarly() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), HostStreamSettings.class)
                .putExtra("host_uuid", "editor-host").putExtra("host_name", "PC");
        ActivityController<HostStreamSettings> controller = Robolectric.buildActivity(HostStreamSettings.class, intent).setup();
        try {
            HostStreamSettings screen = controller.get();
            assertFalse(screen.findViewById(R.id.host_profile_save).isEnabled());
            HostStreamSettings.SettingsFragment fragment = (HostStreamSettings.SettingsFragment)
                    screen.getFragmentManager().findFragmentById(R.id.host_profile_settings);
            fragment.findPreference("bitrate").getOnPreferenceClickListener().onPreferenceClick(fragment.findPreference("bitrate"));
            AlertDialog editor = (AlertDialog) ShadowDialog.getLatestDialog();
            ((android.widget.CompoundButton) editor.findViewById(R.id.host_profile_follow_all)).setChecked(false);
            ((android.widget.EditText) editor.findViewById(R.id.host_profile_field_primary)).setText("45.5");
            assertNotNull(findScrollView((android.view.ViewGroup) editor.findViewById(android.R.id.content)));
            controller.recreate();
            editor = (AlertDialog) ShadowDialog.getLatestDialog();
            assertTrue(editor.isShowing());
            assertEquals("45.5", ((android.widget.EditText) editor.findViewById(R.id.host_profile_field_primary)).getText().toString());
            assertFalse(((android.widget.CompoundButton) editor.findViewById(R.id.host_profile_follow_all)).isChecked());
            assertNull(HostStreamProfile.load(controller.get(), "editor-host"));
            editor.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertTrue(controller.get().findViewById(R.id.host_profile_save).isEnabled());
            controller.get().findViewById(R.id.host_profile_save).performClick();
            assertEquals(Integer.valueOf(45500), HostStreamProfile.load(controller.get(), "editor-host").bitrate);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    private static android.widget.ScrollView findScrollView(android.view.ViewGroup root) {
        if (root instanceof android.widget.ScrollView) return (android.widget.ScrollView) root;
        for (int i = 0; i < root.getChildCount(); i++) {
            android.view.View child = root.getChildAt(i);
            if (child instanceof android.view.ViewGroup) {
                android.widget.ScrollView found = findScrollView((android.view.ViewGroup) child);
                if (found != null) return found;
            }
        }
        return null;
    }
}

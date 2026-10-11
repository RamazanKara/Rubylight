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
    public void oldOverlayChoicesMigrateToStatisticsOrDebug() {
        Application app = RuntimeEnvironment.getApplication();
        SharedPreferences prefs = android.preference.PreferenceManager.getDefaultSharedPreferences(app);
        for (String old : new String[] {"compact", "expanded", "advanced", "statistics", "debug"}) {
            prefs.edit().putString("performance_overlay_mode", old).commit();
            PreferenceConfiguration.readPreferences(app);
            assertEquals("advanced".equals(old) || "debug".equals(old) ? "debug" : "statistics",
                    prefs.getString("performance_overlay_mode", null));
        }
    }

    @Test
    public void explicitDisplayModeIsVisibleFirstAndControlsVirtualDisplay() {
        Application app = RuntimeEnvironment.getApplication();
        HostDisplayChoice.save(app, "exclusive-host", HostDisplayChoice.EXCLUSIVE);
        Intent intent = new Intent(app, HostStreamSettings.class)
                .putExtra("host_uuid", "exclusive-host").putExtra("host_name", "PC");
        ActivityController<HostStreamSettings> controller = Robolectric.buildActivity(HostStreamSettings.class, intent).setup();
        try {
            HostStreamSettings.SettingsFragment fragment = (HostStreamSettings.SettingsFragment)
                    controller.get().getFragmentManager().findFragmentById(R.id.host_profile_settings);
            android.preference.PreferenceGroup first = (android.preference.PreferenceGroup)
                    fragment.getPreferenceScreen().getPreference(0);
            assertEquals("host_display", first.getPreference(0).getKey());
            assertEquals(app.getString(R.string.host_display_exclusive),
                    fragment.findPreference("host_display").getSummary());
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
    public void resolutionAndFrameRateChangesPreserveExplicitBitrateIncludingTheOldDefault() {
        SharedPreferences prefs = RuntimeEnvironment.getApplication().getSharedPreferences("bitrate-test", 0);
        StreamSettings.SettingsFragment fragment = new StreamSettings.SettingsFragment();
        for (int bitrate : new int[] {45500, PreferenceConfiguration.getDefaultBitrate("1280x720", "60")}) {
            prefs.edit().putString(PreferenceConfiguration.RESOLUTION_PREF_STRING, "1280x720")
                    .putString(PreferenceConfiguration.FPS_PREF_STRING, "60")
                    .putInt(PreferenceConfiguration.BITRATE_PREF_STRING, bitrate).commit();
            ReflectionHelpers.callInstanceMethod(fragment, "resetBitrateToDefault",
                    ReflectionHelpers.ClassParameter.from(SharedPreferences.class, prefs),
                    ReflectionHelpers.ClassParameter.from(String.class, "1920x1080"),
                    ReflectionHelpers.ClassParameter.from(String.class, "120"));
            assertEquals(bitrate, prefs.getInt(PreferenceConfiguration.BITRATE_PREF_STRING, 0));
        }
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
}

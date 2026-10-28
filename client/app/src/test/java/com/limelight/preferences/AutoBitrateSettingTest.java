package com.limelight.preferences;

import android.app.Application;
import android.content.Context;
import android.content.SharedPreferences;
import android.preference.Preference;
import android.preference.PreferenceGroup;
import android.preference.PreferenceManager;
import android.preference.SwitchPreference;

import com.limelight.R;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;

import static com.limelight.preferences.SettingsStructureTest.open;
import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class AutoBitrateSettingTest {
    private static Application app() {
        return RuntimeEnvironment.getApplication();
    }

    private static SharedPreferences prefs() {
        return PreferenceManager.getDefaultSharedPreferences(app());
    }

    private static boolean on(String store, String pc) {
        return AutoBitrateChoice.isOn(app(), store, pc);
    }

    @After
    public void clear() {
        prefs().edit().clear().commit();
        for (String store : new String[] {AutoBitrateChoice.CODEC_STORE, AutoBitrateChoice.PYROWAVE_STORE}) {
            app().getSharedPreferences(store, Context.MODE_PRIVATE).edit().clear().commit();
        }
    }

    @Test
    public void theOldMinimumAndMaximumAreDeletedWhenSettingsLoad() {
        prefs().edit().putString("list_auto_bitrate_min_kbps", "20000")
                .putString("list_auto_bitrate_max_kbps", "50000").putString("list_fps", "60").commit();
        PreferenceConfiguration.readPreferences(app());
        assertFalse(prefs().contains("list_auto_bitrate_min_kbps"));
        assertFalse(prefs().contains("list_auto_bitrate_max_kbps"));
        assertEquals("Other settings stay", "60", prefs().getString("list_fps", null));
        // A second load has nothing left to do.
        PreferenceConfiguration.readPreferences(app());
        assertFalse(prefs().contains("list_auto_bitrate_min_kbps"));
    }

    @Test
    public void theOldListsAreNotInTheSettingsScreen() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings.SettingsFragment video = open(controller.get(), "video");
            assertNull(video.findPreference("list_auto_bitrate_min_kbps"));
            assertNull(video.findPreference("list_auto_bitrate_max_kbps"));
        }
    }

    @Test
    public void theSwitchSitsDirectlyUnderTheBitrateSliderAndIsOffByDefault() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings.SettingsFragment video = open(controller.get(), "video");
            SwitchPreference auto = (SwitchPreference) video.findPreference(AutoBitrateChoice.SETTINGS_KEY);
            assertNotNull(auto);
            assertFalse(auto.isChecked());
            assertEquals(app().getString(R.string.title_auto_bitrate), auto.getTitle().toString());
            Preference slider = video.findPreference("seekbar_bitrate_kbps");
            PreferenceGroup group = (PreferenceGroup) video.getPreferenceScreen().getPreference(0);
            int at = -1;
            for (int i = 0; i < group.getPreferenceCount(); i++) {
                if (group.getPreference(i) == slider) at = i;
            }
            assertTrue(at >= 0);
            // The loss-based switch (0x553C) lives in Advanced's Experimental group (SettingsTiersTest), so nothing
            // sits between.
            assertSame(auto, group.getPreference(at + 1));
            assertFalse(PreferenceConfiguration.readPreferences(app()).autoBitrate);
        }
    }

    @Test
    public void thePcsFollowTheSwitchUntilTheStreamMenuChoosesForOne() {
        assertFalse(on(AutoBitrateChoice.CODEC_STORE, "a"));
        assertFalse("No PC id: the default", on(AutoBitrateChoice.CODEC_STORE, null));
        prefs().edit().putBoolean(AutoBitrateChoice.SETTINGS_KEY, true).commit();
        assertTrue(on(AutoBitrateChoice.CODEC_STORE, "a"));
        assertTrue(on(AutoBitrateChoice.PYROWAVE_STORE, "a"));
        assertTrue(PreferenceConfiguration.readPreferences(app()).autoBitrate);

        // The stream menu turns it off for A only, in the store of the stream's codec.
        AutoBitrateChoice.setForPc(app(), AutoBitrateChoice.CODEC_STORE, "a", false);
        assertFalse(on(AutoBitrateChoice.CODEC_STORE, "a"));
        assertTrue(on(AutoBitrateChoice.PYROWAVE_STORE, "a"));
        assertTrue(on(AutoBitrateChoice.CODEC_STORE, "b"));
    }

    @Test
    public void changingTheSettingsSwitchMakesEveryPcFollowItAgain() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings.SettingsFragment video = open(controller.get(), "video");
            SwitchPreference auto = (SwitchPreference) video.findPreference(AutoBitrateChoice.SETTINGS_KEY);
            AutoBitrateChoice.setForPc(app(), AutoBitrateChoice.CODEC_STORE, "a", true);
            AutoBitrateChoice.setForPc(app(), AutoBitrateChoice.PYROWAVE_STORE, "b", true);
            assertTrue(on(AutoBitrateChoice.CODEC_STORE, "a"));

            // Off in Settings: the PCs the stream menu had switched on follow it.
            assertTrue(auto.getOnPreferenceChangeListener().onPreferenceChange(auto, false));
            auto.setChecked(false);
            assertFalse(on(AutoBitrateChoice.CODEC_STORE, "a"));
            assertFalse(on(AutoBitrateChoice.PYROWAVE_STORE, "b"));

            // On in Settings, then off again for one PC from the stream menu.
            assertTrue(auto.getOnPreferenceChangeListener().onPreferenceChange(auto, true));
            auto.setChecked(true);
            assertTrue(on(AutoBitrateChoice.CODEC_STORE, "a"));
            assertTrue(on(AutoBitrateChoice.PYROWAVE_STORE, "b"));
            AutoBitrateChoice.setForPc(app(), AutoBitrateChoice.CODEC_STORE, "a", false);
            assertFalse(on(AutoBitrateChoice.CODEC_STORE, "a"));
            assertTrue(on(AutoBitrateChoice.CODEC_STORE, "b"));
        }
    }

    @Test
    public void aRemovedPcLeavesNoChoiceBehind() {
        prefs().edit().putBoolean(AutoBitrateChoice.SETTINGS_KEY, true).commit();
        AutoBitrateChoice.setForPc(app(), AutoBitrateChoice.CODEC_STORE, "a", false);
        AutoBitrateChoice.setForPc(app(), AutoBitrateChoice.PYROWAVE_STORE, "a", false);
        assertFalse(on(AutoBitrateChoice.CODEC_STORE, "a"));
        AutoBitrateChoice.forget(app(), "a");
        assertTrue(on(AutoBitrateChoice.CODEC_STORE, "a"));
        assertTrue(on(AutoBitrateChoice.PYROWAVE_STORE, "a"));
    }

    @Test
    public void onlyAChoiceThatDiffersFromTheSettingsSwitchIsStored() {
        SharedPreferences codec = app().getSharedPreferences(AutoBitrateChoice.CODEC_STORE, Context.MODE_PRIVATE);
        // Switch off: turning it on for a PC is an override, turning it off again is not.
        AutoBitrateChoice.setForPc(app(), AutoBitrateChoice.CODEC_STORE, "a", true);
        assertTrue(codec.contains("a"));
        AutoBitrateChoice.setForPc(app(), AutoBitrateChoice.CODEC_STORE, "a", false);
        assertFalse(codec.contains("a"));
        // Switch on: the other way round.
        prefs().edit().putBoolean(AutoBitrateChoice.SETTINGS_KEY, true).commit();
        AutoBitrateChoice.setForPc(app(), AutoBitrateChoice.CODEC_STORE, "a", true);
        assertFalse(codec.contains("a"));
        AutoBitrateChoice.setForPc(app(), AutoBitrateChoice.CODEC_STORE, "a", false);
        assertTrue(codec.contains("a"));
        assertFalse(on(AutoBitrateChoice.CODEC_STORE, "a"));
    }
}

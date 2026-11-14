package com.limelight.preferences;

import android.app.Application;
import android.content.Intent;
import android.content.SharedPreferences;
import android.preference.Preference;
import android.preference.PreferenceManager;
import android.view.Display;

import com.limelight.R;
import com.limelight.binding.video.DisplayFrameRatePolicy;
import com.limelight.computers.HostCapabilityCache;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.TestHosts;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.shadows.ShadowDisplay;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

/** Adaptive refresh is offered only where a screen has ARR; elsewhere it is greyed and reads as off. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 36, application = Application.class,
        shadows = {VrrOfferedTest.ArrDisplay.class, com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class VrrOfferedTest {
    @Implements(Display.class)
    public static class ArrDisplay extends ShadowDisplay {
        static boolean supported;

        @Implementation protected boolean hasArrSupport() {
            return supported;
        }
    }

    @After
    public void reset() {
        ArrDisplay.supported = false;
        prefs().edit().clear().commit();
    }

    private static SharedPreferences prefs() {
        return PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication());
    }

    @Test
    public void aScreenWithArrOffersVrrAndTheSavedChoiceHolds() {
        ArrDisplay.supported = true;
        prefs().edit().putBoolean(PreferenceConfiguration.VRR_PREF_STRING, true).commit();
        assertTrue(DisplayFrameRatePolicy.vrrOffered(RuntimeEnvironment.getApplication()));
        assertTrue(PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication()).vrr);
    }

    @Test
    public void withoutArrASavedVrrReadsAsOff() {
        prefs().edit().putBoolean(PreferenceConfiguration.VRR_PREF_STRING, true).commit();
        assertFalse(DisplayFrameRatePolicy.vrrOffered(RuntimeEnvironment.getApplication()));
        assertFalse(PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication()).vrr);
    }

    @Test
    @Config(sdk = 35)
    public void androidBefore16NeverOffersVrr() {
        ArrDisplay.supported = true;
        assertFalse(DisplayFrameRatePolicy.vrrOffered(RuntimeEnvironment.getApplication()));
    }

    @Test
    public void withoutArrTheSettingIsHiddenAndPacingShows() {
        prefs().edit().putBoolean(PreferenceConfiguration.VRR_PREF_STRING, true).commit();
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings.SettingsFragment display = SettingsStructureTest.open(controller.get(), "display");
            // Shown only where it works: no greyed row and no sentence.
            assertNull(display.findPreference(PreferenceConfiguration.VRR_PREF_STRING));
            java.util.List<String> shown = new java.util.ArrayList<>();
            android.preference.PreferenceScreen screen = display.getPreferenceScreen();
            for (int i = 0; i < screen.getPreferenceCount(); i++) {
                shown.addAll(SettingsStructureTest.keys((android.preference.PreferenceGroup) screen.getPreference(i)));
            }
            assertTrue("frame pacing applies again", shown.contains("frame_pacing"));
            assertTrue("so does the lower refresh rate", shown.contains("checkbox_reduce_refresh_rate"));
            // Another setting's change reruns the rules; the stored VRR choice still doesn't hide pacing.
            prefs().edit().putBoolean(PreferenceConfiguration.HOST_FRAME_TIMES_PREF_STRING, false).commit();
            assertTrue(SettingRules.framePacingApplies(controller.get(), prefs()));
        }
        assertTrue("opening Settings keeps the saved choice for a screen that gains ARR",
                prefs().getBoolean(PreferenceConfiguration.VRR_PREF_STRING, false));
        assertTrue(SettingRules.framePacingApplies(RuntimeEnvironment.getApplication(), prefs()));
        ArrDisplay.supported = true;
        assertTrue(PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication()).vrr);
        assertFalse(SettingRules.framePacingApplies(RuntimeEnvironment.getApplication(), prefs()));
    }

    @Test
    public void withoutArrAPcOrGameLayerCantTurnVrrOn() {
        prefs().edit().putString(PreferenceConfiguration.FRAME_PACING_PREF_STRING, "balanced")
                .putBoolean("checkbox_reduce_refresh_rate", true).commit();
        HostStreamProfile.deserialize("v2;vrr=true").save(RuntimeEnvironment.getApplication(), "vrr-pc");
        AppStreamSettings.save(RuntimeEnvironment.getApplication(), "plain-pc", "game", 1,
                HostStreamProfile.deserialize("v2;vrr=true"));
        for (String[] stream : new String[][] {{"vrr-pc", "desktop"}, {"plain-pc", "game"}}) {
            PreferenceConfiguration config = stream(stream[0], stream[1]);
            assertFalse(stream[0], config.vrr);
            assertEquals("pacing stays the chosen one", PreferenceConfiguration.FRAME_PACING_BALANCED, config.framePacing);
            assertTrue("the lower refresh rate stays allowed", config.reduceRefreshRate);
        }
        ArrDisplay.supported = true;
        assertTrue(stream("vrr-pc", "desktop").vrr);
        assertTrue(stream("plain-pc", "game").vrr);
    }

    @Test
    public void realVrrIsOnWithVrrAndTheFixedRefreshSwitchKeepsItsOwnChoice() {
        Application app = RuntimeEnvironment.getApplication();
        ArrDisplay.supported = true;
        assertFalse("without VRR the fixed refresh's switch decides, off by default",
                PreferenceConfiguration.readPreferences(app).hostFrameTimes);
        // The old switch's off, which first launches stored as its default, doesn't turn Real VRR off.
        prefs().edit().putBoolean(PreferenceConfiguration.VRR_PREF_STRING, true)
                .putBoolean(PreferenceConfiguration.HOST_FRAME_TIMES_PREF_STRING, false).commit();
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(app);
        assertTrue("Real VRR is the default with VRR", config.hostFrameTimes);
        assertEquals(PreferenceConfiguration.FRAME_PACING_MIN_LATENCY, config.framePacing);
        prefs().edit().putBoolean(PreferenceConfiguration.REAL_VRR_PREF_STRING, false).commit();
        assertFalse(PreferenceConfiguration.readPreferences(app).hostFrameTimes);
        prefs().edit().putBoolean(PreferenceConfiguration.VRR_PREF_STRING, false)
                .putBoolean(PreferenceConfiguration.HOST_FRAME_TIMES_PREF_STRING, true).commit();
        assertTrue(PreferenceConfiguration.readPreferences(app).hostFrameTimes);
        // A saved VRR reads as off without ARR, so the fixed refresh's switch decides there.
        prefs().edit().putBoolean(PreferenceConfiguration.VRR_PREF_STRING, true)
                .putBoolean(PreferenceConfiguration.REAL_VRR_PREF_STRING, true)
                .putBoolean(PreferenceConfiguration.HOST_FRAME_TIMES_PREF_STRING, false).commit();
        ArrDisplay.supported = false;
        assertFalse(PreferenceConfiguration.readPreferences(app).hostFrameTimes);
    }

    @Test
    public void aPcOrGameLayerThatTurnsVrrOnOrOffPicksTheMatchingSwitch() {
        Application app = RuntimeEnvironment.getApplication();
        ArrDisplay.supported = true;
        HostStreamProfile.deserialize("v2;vrr=true").save(app, "vrr-pc");
        HostStreamProfile.deserialize("v2;vrr=false").save(app, "flat-pc");
        PreferenceConfiguration config = stream("vrr-pc", "desktop");
        assertTrue("a PC that turns VRR on gets Real VRR", config.vrr && config.hostFrameTimes);
        prefs().edit().putBoolean(PreferenceConfiguration.VRR_PREF_STRING, true)
                .putString(PreferenceConfiguration.FRAME_PACING_PREF_STRING, "balanced").commit();
        assertTrue(PreferenceConfiguration.readPreferences(app).hostFrameTimes);
        config = stream("flat-pc", "desktop");
        assertFalse("a PC that turns VRR off gets the fixed refresh's switch", config.vrr || config.hostFrameTimes);
        assertEquals("and its pacing back", PreferenceConfiguration.FRAME_PACING_BALANCED, config.framePacing);
    }

    @Test
    public void vrrOnByDefaultIsRealVrrAndTheFixedRefreshSwitchShowsWithoutVrr() {
        ArrDisplay.supported = true;
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            // The screen holds only the rows shown; the fragment also finds the hidden ones.
            android.preference.PreferenceScreen display =
                    SettingsStructureTest.open(controller.get(), "display").getPreferenceScreen();
            assertNull("Real VRR is the Variable refresh rate row's On", display.findPreference(PreferenceConfiguration.REAL_VRR_PREF_STRING));
            assertTrue(display.findPreference(PreferenceConfiguration.HOST_FRAME_TIMES_PREF_STRING) != null);
            prefs().edit().putBoolean(PreferenceConfiguration.VRR_PREF_STRING, true).commit();
            assertNull(display.findPreference(PreferenceConfiguration.HOST_FRAME_TIMES_PREF_STRING));
            assertEquals("on by default", "on", ((android.preference.ListPreference) display.findPreference(
                    PreferenceConfiguration.VRR_PREF_STRING)).getValue());
        }
    }

    private static PreferenceConfiguration stream(String pc, String game) {
        return PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication(), pc,
                HostCapabilities.UNKNOWN, game, 1, StreamLayers.Network.WIFI);
    }

    @Test
    public void withoutArrThePcRowIsGreyedAndPacingIsntReplaced() {
        HostStreamProfile.deserialize("v2;vrr=true;real_vrr=false").save(RuntimeEnvironment.getApplication(), "vrr-pc");
        ActivityController<HostStreamSettings> controller = openPc("vrr-pc");
        try {
            HostStreamSettings.SettingsFragment settings = (HostStreamSettings.SettingsFragment)
                    controller.get().getFragmentManager().findFragmentById(R.id.host_profile_settings);
            Preference vrr = settings.findPreference("vrr");
            assertFalse(vrr.isEnabled());
            assertEquals(controller.get().getString(R.string.summary_vrr_unavailable), String.valueOf(vrr.getSummary()));
            String pacing = String.valueOf(settings.findPreference("pacing").getSummary());
            assertFalse(pacing.contains(controller.get().getString(R.string.layers_pacing_vrr)));
        } finally {
            controller.pause().stop().destroy();
        }
        HostStreamProfile kept = HostStreamProfile.deserialize(HostStreamProfile.raw(RuntimeEnvironment.getApplication(), "vrr-pc"));
        assertEquals("the PC's choice is kept", Boolean.TRUE, kept.vrr);
        assertEquals("with its Real VRR", Boolean.FALSE, kept.realVrr);

        ArrDisplay.supported = true;
        controller = openPc("vrr-pc");
        try {
            HostStreamSettings.SettingsFragment settings = (HostStreamSettings.SettingsFragment)
                    controller.get().getFragmentManager().findFragmentById(R.id.host_profile_settings);
            assertTrue(settings.findPreference("vrr").isEnabled());
            assertTrue("the PC's Fastest", String.valueOf(settings.findPreference("vrr").getSummary())
                    .startsWith(controller.get().getResources().getStringArray(R.array.vrr_choice_names)[VrrChoice.FASTEST]));
            assertTrue(String.valueOf(settings.findPreference("pacing").getSummary())
                    .contains(controller.get().getString(R.string.layers_pacing_vrr)));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    private static ActivityController<HostStreamSettings> openPc(String uuid) {
        Application app = RuntimeEnvironment.getApplication();
        HostCapabilityCache.save(app, uuid, TestHosts.rubylight());
        Intent intent = new Intent(app, HostStreamSettings.class).putExtra("host_uuid", uuid).putExtra("host_name", "Den PC");
        return Robolectric.buildActivity(HostStreamSettings.class, intent).setup();
    }

    @Test
    public void withoutArrTheRowsChoiceReadsAsOffAndTheSwitchesAreKept() {
        prefs().edit().putBoolean(PreferenceConfiguration.VRR_PREF_STRING, true)
                .putBoolean(PreferenceConfiguration.REAL_VRR_PREF_STRING, false).commit();
        assertEquals(VrrChoice.OFF, VrrChoice.shown(prefs(), DisplayFrameRatePolicy.vrrOffered(RuntimeEnvironment.getApplication())));
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            SettingsStructureTest.open(controller.get(), "display");
        }
        assertTrue(prefs().getBoolean(PreferenceConfiguration.VRR_PREF_STRING, false));
        assertFalse(prefs().getBoolean(PreferenceConfiguration.REAL_VRR_PREF_STRING, true));
        ArrDisplay.supported = true;
        assertEquals(VrrChoice.FASTEST, VrrChoice.shown(prefs(), DisplayFrameRatePolicy.vrrOffered(RuntimeEnvironment.getApplication())));
    }

    @Test
    public void withArrTheSettingCanBeTurnedOn() {
        ArrDisplay.supported = true;
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings.SettingsFragment display = SettingsStructureTest.open(controller.get(), "display");
            assertTrue(display.findPreference(PreferenceConfiguration.VRR_PREF_STRING).isEnabled());
        }
    }
}

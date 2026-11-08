package com.limelight.preferences;

import android.app.Application;
import android.content.Intent;
import android.content.SharedPreferences;
import android.preference.Preference;
import android.preference.PreferenceManager;
import android.preference.SwitchPreference;
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
    public void withoutArrTheSettingIsGreyedOffAndSaysWhy() {
        prefs().edit().putBoolean(PreferenceConfiguration.VRR_PREF_STRING, true).commit();
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings.SettingsFragment display = SettingsStructureTest.open(controller.get(), "display");
            SwitchPreference vrr = (SwitchPreference) display.findPreference(PreferenceConfiguration.VRR_PREF_STRING);
            assertFalse(vrr.isEnabled());
            assertFalse(vrr.isChecked());
            assertEquals(controller.get().getString(R.string.summary_vrr_unavailable), String.valueOf(vrr.getSummary()));
            assertTrue("frame pacing applies again", display.findPreference("frame_pacing").isEnabled());
            assertTrue("so does the lower refresh rate", display.findPreference("checkbox_reduce_refresh_rate").isEnabled());
            // Another setting's change reruns the rules; the stored VRR choice still doesn't grey pacing.
            prefs().edit().putBoolean(PreferenceConfiguration.HOST_FRAME_TIMES_PREF_STRING, false).commit();
            assertTrue(display.findPreference("frame_pacing").isEnabled());
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

    private static PreferenceConfiguration stream(String pc, String game) {
        return PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication(), pc,
                HostCapabilities.UNKNOWN, game, 1, StreamLayers.Network.WIFI);
    }

    @Test
    public void withoutArrThePcRowIsGreyedAndPacingIsntReplaced() {
        HostStreamProfile.deserialize("v2;vrr=true").save(RuntimeEnvironment.getApplication(), "vrr-pc");
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
        assertEquals("the PC's choice is kept", Boolean.TRUE,
                HostStreamProfile.deserialize(HostStreamProfile.raw(RuntimeEnvironment.getApplication(), "vrr-pc")).vrr);

        ArrDisplay.supported = true;
        controller = openPc("vrr-pc");
        try {
            HostStreamSettings.SettingsFragment settings = (HostStreamSettings.SettingsFragment)
                    controller.get().getFragmentManager().findFragmentById(R.id.host_profile_settings);
            assertTrue(settings.findPreference("vrr").isEnabled());
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
    public void withArrTheSettingCanBeTurnedOn() {
        ArrDisplay.supported = true;
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings.SettingsFragment display = SettingsStructureTest.open(controller.get(), "display");
            assertTrue(display.findPreference(PreferenceConfiguration.VRR_PREF_STRING).isEnabled());
        }
    }
}

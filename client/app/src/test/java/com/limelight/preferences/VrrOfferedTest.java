package com.limelight.preferences;

import android.app.Application;
import android.content.SharedPreferences;
import android.preference.PreferenceManager;
import android.preference.SwitchPreference;
import android.view.Display;

import com.limelight.R;
import com.limelight.binding.video.DisplayFrameRatePolicy;

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
            assertFalse(prefs().getBoolean(PreferenceConfiguration.VRR_PREF_STRING, true));
            assertTrue("frame pacing applies again", display.findPreference("frame_pacing").isEnabled());
            assertTrue("so does the lower refresh rate", display.findPreference("checkbox_reduce_refresh_rate").isEnabled());
        }
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

package com.limelight.preferences;

import android.app.Application;
import android.content.SharedPreferences;
import android.preference.PreferenceManager;

import com.limelight.R;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GpuMaxClocksDefaultTest {
    private static final String KEY = PreferenceConfiguration.GPU_MAX_CLOCKS_PREF_STRING;

    private static Application app() {
        return RuntimeEnvironment.getApplication();
    }

    private static SharedPreferences prefs() {
        return PreferenceManager.getDefaultSharedPreferences(app());
    }

    @After
    public void clear() {
        prefs().edit().clear().commit();
    }

    @Test public void aNewInstallBoostsTheGpu() {
        // What the PC list stores on its first launch, followed by the native-defaults migration.
        PreferenceManager.setDefaultValues(app(), R.xml.preferences, true);
        assertTrue(PreferenceConfiguration.readPreferences(app()).gpuMaxClocks);
        assertTrue(prefs().getBoolean(KEY, false));
        assertTrue(PreferenceConfiguration.readPreferences(app()).gpuMaxClocks);
    }

    @Test public void anInstallThatNeverStoredItReadsOn() {
        assertFalse(prefs().contains(KEY));
        assertTrue(PreferenceConfiguration.readPreferences(app()).gpuMaxClocks);
    }

    @Test public void anExistingChoiceIsKept() {
        for (boolean stored : new boolean[] {true, false}) {
            prefs().edit().clear().putBoolean(KEY, stored)
                    .putInt(PreferenceConfiguration.PREFS_VERSION_PREF_STRING, PreferenceConfiguration.PROFILE_NATIVE_VERSION)
                    .commit();
            assertEquals(stored, PreferenceConfiguration.readPreferences(app()).gpuMaxClocks);
            assertEquals(stored, PreferenceConfiguration.readPreferences(app()).gpuMaxClocks);
            assertEquals(stored, prefs().getBoolean(KEY, !stored));
        }
    }

    @Test public void resettingAllSettingsTurnsItOn() {
        prefs().edit().putBoolean(KEY, false).commit();
        StreamSettings.resetAllSettings(app());
        assertTrue(PreferenceConfiguration.readPreferences(app()).gpuMaxClocks);
        // The latency switches the reset turns back on stay on.
        assertTrue(PreferenceConfiguration.readPreferences(app()).networkPriority);
    }
}

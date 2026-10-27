package com.limelight.preferences;

import android.app.Application;
import android.content.SharedPreferences;
import android.preference.PreferenceManager;

import com.limelight.R;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class RustEngineDefaultTest {
    private static final String KEY = "checkbox_rust_engine";

    private static Application app() {
        return RuntimeEnvironment.getApplication();
    }

    private static SharedPreferences prefs() {
        return PreferenceManager.getDefaultSharedPreferences(app());
    }

    @Test public void aNewInstallStreamsWithTheNewEngine() {
        // What the PC list stores on its first launch.
        PreferenceManager.setDefaultValues(app(), R.xml.preferences, true);
        assertTrue(prefs().getBoolean(KEY, false));
        assertTrue(PreferenceConfiguration.readPreferences(app()).rustEngine);
    }

    @Test public void aSettingThatWasNeverStoredReadsOn() {
        assertFalse(prefs().contains(KEY));
        assertTrue(PreferenceConfiguration.readPreferences(app()).rustEngine);
        assertFalse("reading must not store a choice", prefs().contains(KEY));
    }

    @Test public void theOldStoredDefaultIsSwitchedOnOnceAndTheClassicEngineChoiceIsKept() {
        // The beta default every earlier install stored, untouched.
        prefs().edit().putBoolean(KEY, false).commit();
        assertTrue(PreferenceConfiguration.readPreferences(app()).rustEngine);
        assertTrue(prefs().getBoolean(KEY, false));

        // Choosing the classic engine afterwards is a real choice and survives later reads.
        prefs().edit().putBoolean(KEY, false).commit();
        assertFalse(PreferenceConfiguration.readPreferences(app()).rustEngine);
        assertFalse(PreferenceConfiguration.readPreferences(app()).rustEngine);
        assertFalse(prefs().getBoolean(KEY, true));
    }

    @Test public void anExplicitOnIsKept() {
        prefs().edit().putBoolean(KEY, true).commit();
        assertTrue(PreferenceConfiguration.readPreferences(app()).rustEngine);
        assertTrue(PreferenceConfiguration.readPreferences(app()).rustEngine);
        assertTrue(prefs().getBoolean(KEY, false));
    }

    @Test public void theMigrationReportsOnlyTheSwitchItMade() {
        assertFalse(PreferenceConfiguration.migrateRustEngineDefault(prefs()));
        prefs().edit().remove("rust_engine_default_on").putBoolean(KEY, false).commit();
        assertTrue(PreferenceConfiguration.migrateRustEngineDefault(prefs()));
        assertFalse(PreferenceConfiguration.migrateRustEngineDefault(prefs()));
    }
}

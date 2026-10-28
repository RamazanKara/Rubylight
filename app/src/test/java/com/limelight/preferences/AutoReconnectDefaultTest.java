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
public class AutoReconnectDefaultTest {
    private static final String KEY = "checkbox_auto_reconnect";

    private static Application app() {
        return RuntimeEnvironment.getApplication();
    }

    private static SharedPreferences prefs() {
        return PreferenceManager.getDefaultSharedPreferences(app());
    }

    @Test public void aNewInstallStartsWithRecoveryOn() {
        // What the PC list stores on its first launch.
        PreferenceManager.setDefaultValues(app(), R.xml.preferences, true);
        assertTrue(prefs().getBoolean(KEY, false));
        assertTrue(PreferenceConfiguration.readPreferences(app()).autoReconnect);
    }

    @Test public void theOldStoredDefaultIsSwitchedOnOnceAndALaterChoiceIsKept() {
        // Installs that stored off, including those the earlier migration switched off.
        prefs().edit().putBoolean(KEY, false).putBoolean("auto_reconnect_default_off", true).commit();
        assertTrue(PreferenceConfiguration.readPreferences(app()).autoReconnect);
        assertTrue(prefs().getBoolean(KEY, false));

        // Turning it off in Settings is a real choice and survives later reads.
        prefs().edit().putBoolean(KEY, false).commit();
        assertFalse(PreferenceConfiguration.readPreferences(app()).autoReconnect);
        assertFalse(PreferenceConfiguration.readPreferences(app()).autoReconnect);
        assertFalse(prefs().getBoolean(KEY, true));
    }

    @Test public void anInstallThatNeverStoredItReadsOn() {
        assertFalse(prefs().contains(KEY));
        assertTrue(PreferenceConfiguration.readPreferences(app()).autoReconnect);
        prefs().edit().putBoolean(KEY, false).commit();
        assertFalse(PreferenceConfiguration.readPreferences(app()).autoReconnect);
    }

    @Test public void theMigrationRunsOnce() {
        prefs().edit().putBoolean(KEY, false).commit();
        assertTrue(PreferenceConfiguration.migrateAutoReconnectDefault(prefs()));
        prefs().edit().putBoolean(KEY, false).commit();
        assertFalse(PreferenceConfiguration.migrateAutoReconnectDefault(prefs()));
        assertFalse(prefs().getBoolean(KEY, true));
    }
}

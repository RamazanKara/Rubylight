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

    @Test public void aNewInstallStartsWithRecoveryOff() {
        // What the PC list stores on its first launch.
        PreferenceManager.setDefaultValues(app(), R.xml.preferences, true);
        assertFalse(prefs().getBoolean(KEY, true));
        assertFalse(PreferenceConfiguration.readPreferences(app()).autoReconnect);
    }

    @Test public void theOldStoredDefaultIsSwitchedOffOnceAndALaterChoiceIsKept() {
        prefs().edit().putBoolean(KEY, true).commit();
        assertFalse(PreferenceConfiguration.readPreferences(app()).autoReconnect);
        assertFalse(prefs().getBoolean(KEY, true));

        // Turning it back on in Settings is a real choice and survives later reads.
        prefs().edit().putBoolean(KEY, true).commit();
        assertTrue(PreferenceConfiguration.readPreferences(app()).autoReconnect);
        assertTrue(PreferenceConfiguration.readPreferences(app()).autoReconnect);
    }

    @Test public void anInstallThatNeverStoredItReadsOff() {
        assertFalse(prefs().contains(KEY));
        assertFalse(PreferenceConfiguration.readPreferences(app()).autoReconnect);
        prefs().edit().putBoolean(KEY, true).commit();
        assertTrue(PreferenceConfiguration.readPreferences(app()).autoReconnect);
    }
}

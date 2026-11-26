/** Tests the default and one-time upgrade migration for the submit-hint setting. */
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
public class SubmitHintDefaultTest {
    private static final String KEY = "checkbox_submit_hint";

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

    @Test
    public void aNewInstallStartsWithItOn() {
        PreferenceManager.setDefaultValues(app(), R.xml.preferences, true);
        assertTrue(prefs().getBoolean(KEY, false));
        assertTrue(PreferenceConfiguration.readPreferences(app()).submitHint);
    }

    @Test
    public void theOldStoredDefaultIsSwitchedOnOnceAndALaterChoiceIsKept() {
        prefs().edit().putBoolean(KEY, false).commit();
        assertTrue(PreferenceConfiguration.readPreferences(app()).submitHint);
        assertTrue(prefs().getBoolean(KEY, false));

        prefs().edit().putBoolean(KEY, false).commit();
        assertFalse(PreferenceConfiguration.readPreferences(app()).submitHint);
        assertFalse(PreferenceConfiguration.readPreferences(app()).submitHint);
        assertFalse(prefs().getBoolean(KEY, true));
    }

    @Test
    public void theMigrationRunsOnce() {
        prefs().edit().putBoolean(KEY, false).commit();
        assertTrue(PreferenceConfiguration.migrateSubmitHintDefault(prefs()));
        prefs().edit().putBoolean(KEY, false).commit();
        assertFalse(PreferenceConfiguration.migrateSubmitHintDefault(prefs()));
        assertFalse(prefs().getBoolean(KEY, true));
    }
}

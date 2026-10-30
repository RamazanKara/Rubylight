package com.limelight.preferences;

import android.app.Application;
import android.content.SharedPreferences;
import android.preference.PreferenceManager;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import static org.junit.Assert.assertEquals;

/** The bitrate setting's default follows the codec: PyroWave shows its own. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class PyroWaveDefaultBitrateTest {
    private static SharedPreferences prefs() {
        return PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication());
    }

    @After
    public void clear() {
        prefs().edit().clear().commit();
    }

    @Test
    public void pyroWaveShowsItsOwnDefault() {
        assertEquals(398132, PreferenceConfiguration.getShownDefaultBitrate(prefs(), "1920x1080", "60", "forcepyrowave"));
        assertEquals(1415578, PreferenceConfiguration.getShownDefaultBitrate(prefs(), "2560x1440", "120", "forcepyrowave"));
        assertEquals(1592525, PreferenceConfiguration.getShownDefaultBitrate(prefs(), "3840x2160", "60", "forcepyrowave"));
    }

    @Test
    public void otherCodecsKeepTheirDefault() {
        for (String codec : new String[] {"auto", "forceh265", "forceav1"}) {
            assertEquals(PreferenceConfiguration.getDefaultBitrate("1920x1080", "60"),
                    PreferenceConfiguration.getShownDefaultBitrate(prefs(), "1920x1080", "60", codec));
        }
    }

    @Test
    public void choosingPyroWaveMovesADefaultBitrateButNotAChosenOne() {
        prefs().edit().putString(PreferenceConfiguration.RESOLUTION_PREF_STRING, "1920x1080")
                .putString(PreferenceConfiguration.FPS_PREF_STRING, "60").commit();
        StreamSettings.SettingsFragment.resetBitrateToDefault(prefs(), null, null, "forcepyrowave");
        assertEquals(398132, prefs().getInt(PreferenceConfiguration.BITRATE_PREF_STRING, 0));
        StreamSettings.SettingsFragment.resetBitrateToDefault(prefs(), null, null, "auto");
        assertEquals(PreferenceConfiguration.getDefaultBitrate("1920x1080", "60"),
                prefs().getInt(PreferenceConfiguration.BITRATE_PREF_STRING, 0));
    }
}

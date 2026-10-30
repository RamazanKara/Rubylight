package com.limelight.preferences;

import android.app.Application;
import android.content.Intent;
import android.content.SharedPreferences;
import android.preference.PreferenceManager;

import com.limelight.R;
import com.limelight.computers.HostCapabilityCache;
import com.limelight.nvstream.http.TestHosts;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

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
        // The link is not known in the settings: within 900 Mbps, unless the floor is higher.
        assertEquals(900000, PreferenceConfiguration.getShownDefaultBitrate(prefs(), "2560x1440", "120", "forcepyrowave"));
        assertEquals(900000, PreferenceConfiguration.getShownDefaultBitrate(prefs(), "3840x2160", "60", "forcepyrowave"));
        assertEquals(1492992, PreferenceConfiguration.getShownDefaultBitrate(prefs(), "3840x2160", "120", "forcepyrowave"));
    }

    @Test
    public void otherCodecsKeepTheirDefault() {
        for (String codec : new String[] {"auto", "forceh265", "forceav1"}) {
            assertEquals(PreferenceConfiguration.getDefaultBitrate("1920x1080", "60"),
                    PreferenceConfiguration.getShownDefaultBitrate(prefs(), "1920x1080", "60", codec));
        }
    }

    // A chosen bitrate below PyroWave's minimum is kept; the bitrate setting warns, naming the
    // minimum for the current mode.
    @Test
    public void aChosenBitrateBelowPyroWavesMinimumIsKeptAndWarned() {
        prefs().edit().putString(PreferenceConfiguration.RESOLUTION_PREF_STRING, "1920x1080")
                .putString(PreferenceConfiguration.FPS_PREF_STRING, "60")
                .putString(PreferenceConfiguration.VIDEO_FORMAT_PREF_STRING, "forcepyrowave")
                .putInt(PreferenceConfiguration.BITRATE_PREF_STRING, 20000)
                .putBoolean(PreferenceConfiguration.BITRATE_USER_SET_PREF_STRING, true)
                .putInt(PreferenceConfiguration.PREFS_VERSION_PREF_STRING, PreferenceConfiguration.PROFILE_NATIVE_VERSION)
                .commit();
        assertEquals(186624, minimumAboveChosen());
        assertEquals(20000, PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication()).bitrate);
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings.SettingsFragment video = SettingsStructureTest.open(controller.get(), "video");
            String summary = String.valueOf(video.findPreference(PreferenceConfiguration.BITRATE_PREF_STRING).getSummary());
            assertTrue(summary, summary.contains(RuntimeEnvironment.getApplication()
                    .getString(R.string.summary_bitrate_pyrowave_below_minimum, 187, 1920, 1080, 60)));
        }

        // At or above the minimum, with another codec, or automatic: no warning.
        prefs().edit().putInt(PreferenceConfiguration.BITRATE_PREF_STRING, 186624).commit();
        assertEquals(0, minimumAboveChosen());
        prefs().edit().putInt(PreferenceConfiguration.BITRATE_PREF_STRING, 20000)
                .putString(PreferenceConfiguration.VIDEO_FORMAT_PREF_STRING, "forceh265").commit();
        assertEquals(0, minimumAboveChosen());
        prefs().edit().putString(PreferenceConfiguration.VIDEO_FORMAT_PREF_STRING, "forcepyrowave")
                .putBoolean(PreferenceConfiguration.BITRATE_USER_SET_PREF_STRING, false).commit();
        assertEquals(0, minimumAboveChosen());
    }

    private static int minimumAboveChosen() {
        return PreferenceConfiguration.pyroWaveMinimumAboveChosenKbps(
                PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication()));
    }

    // The warning measures the size the stream gets: a resolution that follows the screen is not
    // judged by the saved 4K one.
    @Test
    public void theWarningUsesTheSizeThatFollowsTheScreen() {
        prefs().edit().putString(PreferenceConfiguration.RESOLUTION_PREF_STRING, "3840x2160")
                .putBoolean(PreferenceConfiguration.RESOLUTION_FOLLOWS_SCREEN_PREF_STRING, true)
                .putString(PreferenceConfiguration.FPS_PREF_STRING, "60")
                .putString(PreferenceConfiguration.VIDEO_FORMAT_PREF_STRING, "forcepyrowave")
                .putInt(PreferenceConfiguration.BITRATE_PREF_STRING, 700000)
                .putBoolean(PreferenceConfiguration.BITRATE_USER_SET_PREF_STRING, true)
                .putInt(PreferenceConfiguration.PREFS_VERSION_PREF_STRING, PreferenceConfiguration.PROFILE_NATIVE_VERSION)
                .commit();
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication());
        assertTrue(config.width + "x" + config.height, config.height < 2160);
        int minimum = com.limelight.nvstream.PyroWaveBitrateFloor.minimumKbps(config.width, config.height, config.fps);
        assertEquals(700000 < minimum ? minimum : 0, minimumAboveChosen());
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings.SettingsFragment video = SettingsStructureTest.open(controller.get(), "video");
            String summary = String.valueOf(video.findPreference(PreferenceConfiguration.BITRATE_PREF_STRING).getSummary());
            assertFalse(summary, summary.contains(RuntimeEnvironment.getApplication()
                    .getString(R.string.summary_bitrate_pyrowave_below_minimum, 747, 3840, 2160, 60)));
        }
    }

    // PyroWave and a bitrate picked in a PC's layer warn on that PC's screen, also when the main
    // settings stream another codec at an automatic bitrate.
    @Test
    public void aPcLayerThatPicksPyroWaveWarnsOnItsScreen() {
        Application app = RuntimeEnvironment.getApplication();
        prefs().edit().putString(PreferenceConfiguration.RESOLUTION_PREF_STRING, "1920x1080")
                .putString(PreferenceConfiguration.FPS_PREF_STRING, "60")
                .putString(PreferenceConfiguration.VIDEO_FORMAT_PREF_STRING, "forceh265")
                .putInt(PreferenceConfiguration.PREFS_VERSION_PREF_STRING, PreferenceConfiguration.PROFILE_NATIVE_VERSION)
                .commit();
        String warning = app.getString(R.string.summary_bitrate_pyrowave_below_minimum, 187, 1920, 1080, 60);

        HostStreamProfile layer = new HostStreamProfile();
        layer.codec = PreferenceConfiguration.FormatOption.FORCE_PYROWAVE;
        layer.bitrate = 50000;
        layer.save(app, "pyrowave-pc");
        HostCapabilityCache.save(app, "pyrowave-pc", TestHosts.rubylight());
        Intent pc = new Intent(app, HostStreamSettings.class).putExtra("host_uuid", "pyrowave-pc").putExtra("host_name", "Den PC");
        Intent game = new Intent(app, HostStreamSettings.class).putExtra(HostStreamSettings.EXTRA_MODE, "GAME")
                .putExtra("host_uuid", "pyrowave-pc").putExtra("host_name", "Den PC")
                .putExtra(HostStreamSettings.EXTRA_APP_UUID, "game").putExtra(HostStreamSettings.EXTRA_APP_ID, 4)
                .putExtra(HostStreamSettings.EXTRA_APP_NAME, "Racer");
        assertTrue(bitrateSummary(pc), bitrateSummary(pc).contains(warning));
        // A game of that PC streams the PC's values: its screen warns too.
        assertTrue(bitrateSummary(game), bitrateSummary(game).contains(warning));

        // The PC's PyroWave at the automatic bitrate (which never goes below the minimum): no warning.
        layer.bitrate = null;
        layer.save(app, "pyrowave-pc");
        assertFalse(bitrateSummary(pc), bitrateSummary(pc).contains(warning));
        assertFalse(bitrateSummary(game), bitrateSummary(game).contains(warning));
    }

    private static String bitrateSummary(Intent intent) {
        try (ActivityController<HostStreamSettings> controller = Robolectric.buildActivity(HostStreamSettings.class, intent).setup()) {
            HostStreamSettings.SettingsFragment settings = (HostStreamSettings.SettingsFragment)
                    controller.get().getFragmentManager().findFragmentById(R.id.host_profile_settings);
            return String.valueOf(settings.findPreference("bitrate").getSummary());
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

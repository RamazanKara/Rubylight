package com.limelight.preferences;

import android.app.Application;
import android.content.SharedPreferences;
import android.preference.PreferenceManager;

import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.TestHosts;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import static com.limelight.preferences.PreferenceConfiguration.FormatOption.*;
import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class PyroWaveHostFallbackTest {
    private final Application app = RuntimeEnvironment.getApplication();

    private void allPcs(String codec) {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(app);
        prefs.edit().putString(PreferenceConfiguration.VIDEO_FORMAT_PREF_STRING, codec)
                .putBoolean("checkbox_texture_view", true).commit();
    }

    @Test
    public void pyroWaveForAllPcsFallsBackToTheDeviceChoiceOnHostsWithoutIt() {
        allPcs("forcepyrowave");
        for (String host : new String[] {"sunshine", "apollo", "gamestream"}) {
            PreferenceConfiguration config = PreferenceConfiguration.readPreferences(app, "pc",
                    TestHosts.fixture(host + "-serverinfo.xml"));
            assertTrue(host, config.pyroWaveUnavailable);
            assertEquals(host, AUTO, config.videoFormat);
            // The PyroWave-only restriction no longer costs TextureView.
            assertTrue(host, config.useTextureView);
        }
    }

    @Test
    public void aPcsOwnPyroWaveChoiceFallsBackToTheSettingForAllPcs() {
        allPcs("forceh265");
        HostStreamProfile profile = new HostStreamProfile();
        profile.codec = FORCE_PYROWAVE;
        profile.save(app, "pc");
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(app, "pc",
                TestHosts.fixture("sunshine-serverinfo.xml"));
        assertTrue(config.pyroWaveUnavailable);
        assertEquals(FORCE_HEVC, config.videoFormat);
    }

    @Test
    public void hostsThatMayHavePyroWaveKeepIt() {
        allPcs("forcepyrowave");
        for (HostCapabilities caps : new HostCapabilities[] {TestHosts.rubylight(), TestHosts.fixture("vibepollo-serverinfo.xml"),
                TestHosts.fixture("vibepollo-http-serverinfo.xml"), HostCapabilities.UNKNOWN}) {
            PreferenceConfiguration config = PreferenceConfiguration.readPreferences(app, "pc", caps);
            assertFalse(caps.toString(), config.pyroWaveUnavailable);
            assertEquals(caps.toString(), FORCE_PYROWAVE, config.videoFormat);
            assertFalse(caps.toString(), config.useTextureView);
        }
    }

    @Test
    public void otherCodecsAreUntouched() {
        allPcs("forceav1");
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(app, "pc",
                TestHosts.fixture("sunshine-serverinfo.xml"));
        assertFalse(config.pyroWaveUnavailable);
        assertEquals(FORCE_AV1, config.videoFormat);
    }
}

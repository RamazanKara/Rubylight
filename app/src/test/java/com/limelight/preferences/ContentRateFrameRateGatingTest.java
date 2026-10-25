package com.limelight.preferences;

import android.app.Application;
import android.content.SharedPreferences;
import android.preference.PreferenceManager;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class ContentRateFrameRateGatingTest {
    private static final String KEY = "checkbox_content_rate_frame_rate";

    private static Application app() {
        return RuntimeEnvironment.getApplication();
    }

    private static SharedPreferences.Editor edit() {
        return PreferenceManager.getDefaultSharedPreferences(app()).edit();
    }

    @Test public void theSettingIsOffByDefaultAndFollowsTheSwitch() {
        assertFalse(PreferenceConfiguration.readPreferences(app()).contentRateFrameRate);
        edit().putBoolean(KEY, true).putBoolean("checkbox_reduce_refresh_rate", true).commit();
        assertTrue(PreferenceConfiguration.readPreferences(app()).contentRateFrameRate);
    }

    @Test public void withoutTheLowerRefreshRateTheStoredChoiceDoesNothing() {
        // The settings screen greys the switch without Allow lower refresh rate, so it must not act on its own.
        edit().putBoolean(KEY, true).putBoolean("checkbox_reduce_refresh_rate", false).commit();
        assertFalse(PreferenceConfiguration.readPreferences(app()).contentRateFrameRate);
    }

    @Test public void vrrTakesTheVoteAwaySoTheRendererDoesNotTrackTheRate() {
        edit().putBoolean(KEY, true).putBoolean("checkbox_reduce_refresh_rate", true)
                .putBoolean("checkbox_vrr", true).commit();
        assertFalse(PreferenceConfiguration.readPreferences(app()).contentRateFrameRate);
    }

    @Test public void textureViewTakesTheVoteAway() {
        edit().putBoolean(KEY, true).putBoolean("checkbox_reduce_refresh_rate", true)
                .putBoolean("checkbox_texture_view", true).commit();
        assertFalse(PreferenceConfiguration.readPreferences(app()).contentRateFrameRate);
    }
}

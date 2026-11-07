package com.limelight.binding.video;

import android.app.Application;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowSystemProperties;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

/** debug.nightjar.* decoder switches: 1 and 0 override, anything else is the default. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class)
public class DecoderDebugSwitchTest {
    @After
    public void tearDown() {
        ShadowSystemProperties.reset();
    }

    @Test
    public void unsetIsTheDefault() {
        assertFalse(MediaCodecHelper.debugSwitch("test_switch", false));
        assertTrue(MediaCodecHelper.debugSwitch("test_switch", true));
    }

    @Test
    public void oneAndZeroOverrideEitherDefault() {
        ShadowSystemProperties.override("debug.nightjar.test_switch", "1");
        assertTrue(MediaCodecHelper.debugSwitch("test_switch", false));
        assertTrue(MediaCodecHelper.debugSwitch("test_switch", true));
        ShadowSystemProperties.override("debug.nightjar.test_switch", "0");
        assertFalse(MediaCodecHelper.debugSwitch("test_switch", false));
        assertFalse(MediaCodecHelper.debugSwitch("test_switch", true));
    }

    @Test
    public void anythingElseIsTheDefault() {
        ShadowSystemProperties.override("debug.nightjar.test_switch", "yes");
        assertFalse(MediaCodecHelper.debugSwitch("test_switch", false));
        assertTrue(MediaCodecHelper.debugSwitch("test_switch", true));
    }
}

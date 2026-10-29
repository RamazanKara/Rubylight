package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;

import com.limelight.R;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

/** PyroWave chosen on a device that cannot run it: the renderer keeps the reason for the stream's notice. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class PyroWaveDeviceRefusalTest {
    @Test
    public void aDeviceWithoutThePyroWaveRendererSaysWhy() {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.videoFormat = PreferenceConfiguration.FormatOption.FORCE_PYROWAVE;
            MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity.get(), prefs, null, 0,
                    false, false, "", null);
            // The JVM has no libpyrowave-renderer.so, as a device that can never run it (32-bit, or
            // an app without the library): the reason is the one the settings list shows.
            assertFalse(renderer.isPyroWaveSupported());
            int reason = PyroWaveDecoderRenderer.neverAvailableReason();
            assertTrue(reason == R.string.pyrowave_decoder_unavailable || reason == R.string.pyrowave_needs_64_bit);
            assertEquals(reason, renderer.getPyroWaveUnavailableReason());
        }
    }

    @Test
    public void anotherCodecChosenHasNoReason() {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity.get(),
                    new PreferenceConfiguration(), null, 0, false, false, "", null);
            assertFalse(renderer.isPyroWaveSupported());
            assertEquals(0, renderer.getPyroWaveUnavailableReason());
        }
    }
}

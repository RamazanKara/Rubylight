package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;

import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.utils.LateFrames;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.assertEquals;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class HealthSampleSkipTest {
    /** Game skips the sample in the background, during a host action or while the last one runs. */
    @Test public void aSkippedHealthSampleEndsItsLateFramesTick() {
        ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create();
        try {
            ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
            MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity.get(), prefs, null, 0,
                    false, false, "", null);
            LateFrames late = ReflectionHelpers.getField(renderer, "lateFrames");
            // 60 fps, the second frame held back 43 ms.
            late.push(0, 0, 0, 60);
            late.push(16_667, 0, 60_000, 60);
            renderer.skipHealthSample();
            assertEquals("the next sample starts a new tick", 0, late.tick());
        } finally {
            activity.close();
        }
    }
}

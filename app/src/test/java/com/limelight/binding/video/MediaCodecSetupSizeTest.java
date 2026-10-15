package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;

import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class MediaCodecSetupSizeTest {
    @Test
    public void setupReportsTheSizeTheHostNegotiated() {
        // A 4K request to a host without 4K is negotiated down to 1080p: scaling, letterboxing and
        // touch have to follow the stream the decoder gets, not the size that was asked for.
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
            prefs.width = 3840;
            prefs.height = 2160;
            MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity.get(), prefs,
                    null, 0, false, false, "", null);
            List<int[]> sizes = new ArrayList<>();
            renderer.setStreamSizeListener((width, height) -> sizes.add(new int[] {width, height}));
            try {
                renderer.setup(MoonBridge.VIDEO_FORMAT_H264, 1920, 1080, 60);
            } finally {
                renderer.stop();
                renderer.cleanup();
            }
            assertEquals(1, sizes.size());
            assertArrayEquals(new int[] {1920, 1080}, sizes.get(0));
            assertArrayEquals(new int[] {1920, 1080}, renderer.getStreamSize());
        }
    }
}

package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.hardware.display.DisplayManager;
import android.preference.PreferenceManager;
import android.view.Display;

import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDisplayManager;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class MediaCodecExternalDisplayTest {
    @Test public void externalPyrowaveRetainsBalancedModeBeforeAndAfterNegotiation() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.framePacing = PreferenceConfiguration.FRAME_PACING_BALANCED;
            int externalId = ShadowDisplayManager.addDisplay("w1920dp-h1080dp");
            Display external = activity.get().getSystemService(DisplayManager.class).getDisplay(externalId);
            MediaCodecDecoderRenderer renderer = renderer(activity.get(), prefs, external);
            prefs.videoFormat = PreferenceConfiguration.FormatOption.FORCE_PYROWAVE;

            // Game applies this policy before the host selects the actual codec.
            assertFalse(renderer.applyOutputPacingFallback());
            assertEquals(PreferenceConfiguration.FRAME_PACING_BALANCED, prefs.framePacing);
            ReflectionHelpers.setField(renderer, "videoFormat", MoonBridge.VIDEO_FORMAT_PYROWAVE);
            assertFalse(renderer.applyOutputPacingFallback());
            assertEquals(PreferenceConfiguration.FRAME_PACING_BALANCED, prefs.framePacing);
            renderer.doFrame(1_000_000_000L);
            PyroWaveDisplayClock clock = ReflectionHelpers.getField(renderer, "pyroWaveDisplayClock");
            assertSame(PyroWaveDisplayClock.Snapshot.UNAVAILABLE, clock.snapshot());

            // A host which falls back to a conventional codec still needs the existing policy.
            ReflectionHelpers.setField(renderer, "videoFormat", MoonBridge.VIDEO_FORMAT_H264);
            assertTrue(renderer.applyOutputPacingFallback());
            assertEquals(PreferenceConfiguration.FRAME_PACING_MIN_LATENCY, prefs.framePacing);
        }
    }

    @Test public void externalBalancedPacingFallsBackWithoutChangingSavedSettingsOrStreamRate() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            PreferenceManager.getDefaultSharedPreferences(activity.get()).edit()
                    .putString("frame_pacing", "balanced").commit();
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.framePacing = PreferenceConfiguration.FRAME_PACING_BALANCED;
            prefs.fps = 120;
            prefs.launchRefreshRateX100 = 12000;
            int externalId = ShadowDisplayManager.addDisplay("w1920dp-h1080dp");
            Display external = activity.get().getSystemService(DisplayManager.class).getDisplay(externalId);
            MediaCodecDecoderRenderer renderer = renderer(activity.get(), prefs, external);

            assertTrue(renderer.applyOutputPacingFallback());
            assertEquals(PreferenceConfiguration.FRAME_PACING_MIN_LATENCY, prefs.framePacing);
            assertFalse(renderer.applyOutputPacingFallback());
            assertEquals(120, prefs.fps);
            assertEquals(12000, prefs.launchRefreshRateX100);
            assertFalse(prefs.vrr);
            assertEquals("balanced", PreferenceManager.getDefaultSharedPreferences(activity.get())
                    .getString("frame_pacing", null));

            // Neither MediaCodec nor PyroWave may interpret phone-vsync timestamps
            // as the external display's phase. No codec, GPU or native stream is started.
            for (int format : new int[] {MoonBridge.VIDEO_FORMAT_H264, MoonBridge.VIDEO_FORMAT_PYROWAVE}) {
                ReflectionHelpers.setField(renderer, "videoFormat", format);
                ReflectionHelpers.setField(renderer, "vsyncTimeNs", 0L);
                renderer.doFrame(1_000_000_000L);
                assertEquals(0L, (long) ReflectionHelpers.getField(renderer, "vsyncTimeNs"));
                // Even a stale phase must never be sent to the native host protocol.
                ReflectionHelpers.setField(renderer, "vsyncTimeNs", 1_000_000_000L);
                ReflectionHelpers.setField(renderer, "vsyncIntervalNs", 16_666_666L);
                ReflectionHelpers.callInstanceMethod(renderer, "reportPhaseSlack",
                        ReflectionHelpers.ClassParameter.from(long.class, 1_005_000_000L));
            }
        }
    }

    @Test public void deviceBalancedAndOtherExternalModesKeepTheirPacing() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.framePacing = PreferenceConfiguration.FRAME_PACING_BALANCED;
            MediaCodecDecoderRenderer local = renderer(activity.get(), prefs,
                    activity.get().getWindowManager().getDefaultDisplay());
            assertFalse(local.applyOutputPacingFallback());
            assertEquals(PreferenceConfiguration.FRAME_PACING_BALANCED, prefs.framePacing);

            int externalId = ShadowDisplayManager.addDisplay("w1920dp-h1080dp");
            Display external = activity.get().getSystemService(DisplayManager.class).getDisplay(externalId);
            MediaCodecDecoderRenderer externalRenderer = renderer(activity.get(), prefs, external);
            for (int mode : new int[] {PreferenceConfiguration.FRAME_PACING_MIN_LATENCY,
                    PreferenceConfiguration.FRAME_PACING_CAP_FPS, PreferenceConfiguration.FRAME_PACING_MAX_SMOOTHNESS}) {
                prefs.framePacing = mode;
                assertFalse(externalRenderer.applyOutputPacingFallback());
                assertEquals(mode, prefs.framePacing);
            }
        }
    }

    private static MediaCodecDecoderRenderer renderer(Activity activity, PreferenceConfiguration prefs, Display display) {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        prefs.upscalingMode = UpscalingPolicy.Mode.OFF;
        prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
        return new MediaCodecDecoderRenderer(activity, prefs, null, 0, false, false, "", null, display);
    }
}

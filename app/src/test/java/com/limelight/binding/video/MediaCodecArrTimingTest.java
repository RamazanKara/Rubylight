package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.hardware.display.DisplayManager;
import android.view.Display;

import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.shadow.api.Shadow;
import org.robolectric.shadows.ShadowDisplay;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 36, application = Application.class,
        shadows = {MediaCodecArrTimingTest.ArrDisplay.class, MediaCodecArrTimingTest.SlackBridge.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class MediaCodecArrTimingTest {
    @Test public void activeModeChangesSuppressFixedLatchReportsAndRequireANewPhaseOnReturn() {
        for (boolean vrr : new boolean[] {false, true}) {
            try (ActivityController<Activity> controller = Robolectric.buildActivity(Activity.class).create()) {
                Activity activity = controller.get();
                Display display = activity.getWindowManager().getDefaultDisplay();
                ArrDisplay capability = Shadow.extract(display);
                capability.supported = false;
                MediaCodecDecoderRenderer renderer = renderer(activity, display, vrr);
                ReflectionHelpers.setField(renderer, "streamDisplay", display);
                DisplayManager.DisplayListener listener = ReflectionHelpers.getField(renderer, "displayListener");
                SlackBridge.reports = 0;
                samplePhase(renderer);
                report(renderer);
                assertEquals(1, SlackBridge.reports);

                capability.supported = true;
                listener.onDisplayChanged(display.getDisplayId() + 1);
                report(renderer);
                assertEquals(2, SlackBridge.reports); // Unrelated output does not reset our timing.

                listener.onDisplayChanged(display.getDisplayId());
                samplePhase(renderer);
                report(renderer);
                assertEquals(2, SlackBridge.reports); // An ARR mode has no fixed latch deadline.

                capability.supported = false;
                listener.onDisplayChanged(display.getDisplayId());
                report(renderer);
                assertEquals(2, SlackBridge.reports); // Discard the old mode's phase.
                samplePhase(renderer);
                report(renderer);
                assertEquals(3, SlackBridge.reports);
            }
        }
    }

    @Test public void arrAtConstructionSuppressesReportsEvenWithVrrPreferenceOff() {
        try (ActivityController<Activity> controller = Robolectric.buildActivity(Activity.class).create()) {
            Display display = controller.get().getWindowManager().getDefaultDisplay();
            ((ArrDisplay) Shadow.extract(display)).supported = true;
            MediaCodecDecoderRenderer renderer = renderer(controller.get(), display, false);
            SlackBridge.reports = 0;
            samplePhase(renderer);
            report(renderer);
            assertEquals(0, SlackBridge.reports);
        }
    }

    @Test @Config(sdk = 35)
    public void olderAndroidDoesNotCallTheNewCapabilityApi() {
        try (ActivityController<Activity> controller = Robolectric.buildActivity(Activity.class).create()) {
            Display display = controller.get().getWindowManager().getDefaultDisplay();
            ArrDisplay capability = Shadow.extract(display);
            capability.supported = true;
            MediaCodecDecoderRenderer renderer = renderer(controller.get(), display, true);
            ReflectionHelpers.setField(renderer, "streamDisplay", display);
            ReflectionHelpers.callInstanceMethod(renderer, "updateDisplayTiming");
            SlackBridge.reports = 0;
            samplePhase(renderer);
            report(renderer);
            assertEquals(1, SlackBridge.reports);
            assertEquals(0, capability.queries);
        }
    }

    private static MediaCodecDecoderRenderer renderer(Activity activity, Display display, boolean vrr) {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.vrr = vrr;
        prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
        prefs.upscalingMode = UpscalingPolicy.Mode.OFF;
        return new MediaCodecDecoderRenderer(activity, prefs, null, 0, false, false, "", null, display);
    }

    private static void samplePhase(MediaCodecDecoderRenderer renderer) {
        ReflectionHelpers.setField(renderer, "vsyncTimeNs", 1_000_000_000L);
        ReflectionHelpers.setField(renderer, "vsyncIntervalNs", 10_000_000L);
        ReflectionHelpers.setField(renderer, "vsyncPresentationDeadlineNs", 2_000_000L);
    }

    private static void report(MediaCodecDecoderRenderer renderer) {
        ReflectionHelpers.callInstanceMethod(renderer, "reportPhaseSlack",
                ReflectionHelpers.ClassParameter.from(long.class, 1_005_000_000L));
    }

    @Implements(Display.class)
    public static class ArrDisplay extends ShadowDisplay {
        boolean supported;
        int queries;

        @Implementation protected boolean hasArrSupport() {
            queries++;
            return supported;
        }
    }

    @Implements(value = MoonBridge.class, isInAndroidSdk = false)
    public static class SlackBridge {
        static int reports;
        @Implementation protected static void __staticInitializer__() { }
        @Implementation protected static void reportFrameSlack(long slackNs, long intervalNs) {
            reports++;
            assertEquals(3_000_000L, slackNs);
            assertEquals(10_000_000L, intervalNs);
        }
    }
}

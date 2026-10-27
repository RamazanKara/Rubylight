package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.hardware.display.DisplayManager;
import android.os.Handler;
import android.os.Looper;
import android.view.Display;

import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.annotation.LooperMode;
import org.robolectric.shadow.api.Shadow;
import org.robolectric.shadows.ShadowDisplay;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 36, application = Application.class,
        shadows = {MediaCodecArrTimingTest.ArrDisplay.class, MediaCodecArrTimingTest.SlackBridge.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
@LooperMode(LooperMode.Mode.PAUSED)
public class MediaCodecArrTimingTest {
    @Before public void resetBridge() {
        SlackBridge.reports = SlackBridge.flushes = 0;
        SlackBridge.reportReady = false;
        SlackBridge.onFlush = null;
    }

    @Test public void completedReportsUseOneDeferredFlushForTheLatestMailboxValue() {
        try (ActivityController<Activity> controller = Robolectric.buildActivity(Activity.class).create()) {
            Display display = controller.get().getWindowManager().getDefaultDisplay();
            MediaCodecDecoderRenderer renderer = renderer(controller.get(), display, false);
            ReflectionHelpers.setField(renderer, "latencyHandler", new Handler(Looper.getMainLooper()));
            samplePhase(renderer);
            report(renderer);
            shadowOf(Looper.getMainLooper()).idle();
            assertEquals(0, SlackBridge.flushes); // Ordinary frame samples do not post work.

            SlackBridge.reportReady = true;
            for (int i = 0; i < 20; i++) report(renderer);
            assertEquals(0, SlackBridge.flushes); // Never send synchronously from rendering.
            shadowOf(Looper.getMainLooper()).idle();
            assertEquals(1, SlackBridge.flushes);
        }
    }

    @Test public void aBlockedFlushAllowsOnlyOneSuccessor() {
        try (ActivityController<Activity> controller = Robolectric.buildActivity(Activity.class).create()) {
            Display display = controller.get().getWindowManager().getDefaultDisplay();
            MediaCodecDecoderRenderer renderer = renderer(controller.get(), display, false);
            ReflectionHelpers.setField(renderer, "latencyHandler", new Handler(Looper.getMainLooper()));
            samplePhase(renderer);
            SlackBridge.reportReady = true;
            SlackBridge.onFlush = () -> {
                // Simulate new windows completing while the current native send is in flight.
                for (int i = 0; i < 20; i++) report(renderer);
                assertEquals(1, SlackBridge.flushes);
            };
            report(renderer);
            shadowOf(Looper.getMainLooper()).idle();
            assertEquals(2, SlackBridge.flushes);
        }
    }

    @Test public void stoppedRendererDiscardsQueuedFlushAndDoesNotCollectAgain() {
        try (ActivityController<Activity> controller = Robolectric.buildActivity(Activity.class).create()) {
            Display display = controller.get().getWindowManager().getDefaultDisplay();
            MediaCodecDecoderRenderer renderer = renderer(controller.get(), display, false);
            ReflectionHelpers.setField(renderer, "latencyHandler", new Handler(Looper.getMainLooper()));
            samplePhase(renderer);
            SlackBridge.reportReady = true;
            report(renderer);
            renderer.prepareForStop();
            shadowOf(Looper.getMainLooper()).idle();
            assertEquals(0, SlackBridge.flushes);
            report(renderer);
            assertEquals(1, SlackBridge.reports);
        }
    }

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
        static int reports, flushes;
        static boolean reportReady;
        static Runnable onFlush;
        @Implementation protected static void __staticInitializer__() { }
        @Implementation protected static boolean reportFrameSlack(long slackNs, long intervalNs) {
            reports++;
            assertEquals(3_000_000L, slackNs);
            assertEquals(10_000_000L, intervalNs);
            return reportReady;
        }
        @Implementation protected static void flushPhaseLockReport() {
            flushes++;
            Runnable action = onFlush;
            onFlush = null;
            if (action != null) action.run();
        }
    }
}

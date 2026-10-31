package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.media.MediaCodec;
import android.media.MediaFormat;

import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.shadows.ShadowMediaCodec;
import org.robolectric.shadows.ShadowSystemClock;
import org.robolectric.util.ReflectionHelpers;

import java.lang.reflect.Field;
import java.time.Duration;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

import static org.junit.Assert.*;

/** The phase lock report with "Show frames at the PC's frame times", as with lowest latency. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = {MediaCodecPhaseReportTest.SlackBridge.class, MediaCodecPhaseReportTest.OneFrameCodec.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class MediaCodecPhaseReportTest {
    private static final long VSYNC_NS = 1_000_000_000L;
    private static final long INTERVAL_NS = 10_000_000L;
    private static final long DEADLINE_NS = 2_000_000L;

    @Before public void resetBridge() {
        SlackBridge.slacks.clear();
        SlackBridge.intervals.clear();
    }

    @Test public void pcFrameTimesReportsThePhaseWithRoomBeforeTheLatch() {
        for (boolean hostFrameTimes : new boolean[] {false, true}) {
            try (ActivityController<Activity> controller = Robolectric.buildActivity(Activity.class).create()) {
                MediaCodecDecoderRenderer renderer = renderer(controller.get(), hostFrameTimes);
                assertEquals("the scheduler places frames only with host frame times", hostFrameTimes,
                        FramePacingPolicy.schedulesFrames(hostFrameTimes, PreferenceConfiguration.FRAME_PACING_MIN_LATENCY));
                SlackBridge.slacks.clear();
                SlackBridge.intervals.clear();
                samplePhase(renderer);
                // Ready 5 ms after a vsync: 3 ms before the next latch deadline, and the scheduler
                // asks for 1 ms more room before it.
                report(renderer, VSYNC_NS + 5_000_000L);
                long expected = hostFrameTimes ? 3_000_000L - MediaCodecDecoderRenderer.SCHEDULED_LATCH_ROOM_NS : 3_000_000L;
                assertEquals(Collections.singletonList(expected), SlackBridge.slacks);
                assertEquals(Collections.singletonList(INTERVAL_NS), SlackBridge.intervals);
            }
        }
    }

    @Test public void pcFrameTimesStillSkipsAdaptiveRefreshUpscalingAndAMissingVsync() throws Exception {
        try (ActivityController<Activity> controller = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = renderer(controller.get(), true);
            samplePhase(renderer);

            ReflectionHelpers.setField(renderer, "outputHasArrSupport", true);
            report(renderer, VSYNC_NS + 5_000_000L);
            assertTrue("adaptive refresh has no fixed latch", SlackBridge.slacks.isEmpty());
            ReflectionHelpers.setField(renderer, "outputHasArrSupport", false);

            ReflectionHelpers.setField(renderer, "upscaler", unconstructed(GlesUpscaler.class));
            report(renderer, VSYNC_NS + 5_000_000L);
            assertTrue("an upscaled frame is not display-ready", SlackBridge.slacks.isEmpty());
            ReflectionHelpers.setField(renderer, "upscaler", null);

            ReflectionHelpers.setField(renderer, "outputVsyncAvailable", false);
            report(renderer, VSYNC_NS + 5_000_000L);
            assertTrue("no vsync of the output display", SlackBridge.slacks.isEmpty());
            ReflectionHelpers.setField(renderer, "outputVsyncAvailable", true);

            ReflectionHelpers.setField(renderer, "vsyncTimeNs", 0L);
            report(renderer, VSYNC_NS + 5_000_000L);
            assertTrue("no phase before the first vsync", SlackBridge.slacks.isEmpty());

            samplePhase(renderer);
            report(renderer, VSYNC_NS + 5_000_000L);
            assertEquals("reported once nothing stops it",
                    Collections.singletonList(3_000_000L - MediaCodecDecoderRenderer.SCHEDULED_LATCH_ROOM_NS), SlackBridge.slacks);
        }
    }

    @Test public void aScheduledFrameReportsThePhaseOfItsReadyTimeNotOfItsDueTime() throws Exception {
        try (ActivityController<Activity> controller = Robolectric.buildActivity(Activity.class).create()) {
            ShadowSystemClock.advanceBy(Duration.ofSeconds(2));
            MediaCodecDecoderRenderer renderer = renderer(controller.get(), true);
            renderer.setStreamFrameRateMilliHz(60_000);
            OneFrameCodec.renderer = renderer;
            OneFrameCodec.stats = ReflectionHelpers.getField(renderer, "frameLatencyStats");
            OneFrameCodec.dequeues = OneFrameCodec.dropped = 0;
            OneFrameCodec.releasedAtNs = OneFrameCodec.dueNs = 0;
            MediaCodec codec = MediaCodec.createDecoderByType("video/avc");
            try {
                ReflectionHelpers.setField(renderer, "videoDecoder", codec);
                // Refreshes long enough that the test's own delays stay inside one. With no host
                // time the scheduler stamps the frame due at its ready time plus the deadline.
                long vsyncNs = System.nanoTime();
                long intervalNs = 10_000_000_000L;
                long deadlineNs = 4_000_000_000L;
                ReflectionHelpers.setField(renderer, "vsyncTimeNs", vsyncNs);
                ReflectionHelpers.setField(renderer, "vsyncIntervalNs", intervalNs);
                ReflectionHelpers.setField(renderer, "vsyncPresentationDeadlineNs", deadlineNs);
                ReflectionHelpers.callInstanceMethod(renderer, "startRendererThread");
                Thread thread = ReflectionHelpers.getField(renderer, "rendererThread");
                thread.join(5000);
                long doneNs = System.nanoTime();
                assertFalse("Renderer did not finish its output turn", thread.isAlive());

                assertEquals(0, OneFrameCodec.dropped);
                assertTrue("released ahead of its due time",
                        OneFrameCodec.dueNs - OneFrameCodec.releasedAtNs > deadlineNs / 2);
                assertEquals(1, SlackBridge.slacks.size());
                long slackNs = SlackBridge.slacks.get(0);
                // Ready after the release call returned and before the renderer thread ended.
                long latchNs = deadlineNs + MediaCodecDecoderRenderer.SCHEDULED_LATCH_ROOM_NS;
                assertTrue("slack " + slackNs, slackNs <= MediaCodecDecoderRenderer.latchSlackNs(
                        OneFrameCodec.releasedAtNs, vsyncNs, intervalNs, latchNs));
                assertTrue("slack " + slackNs, slackNs >= MediaCodecDecoderRenderer.latchSlackNs(
                        doneNs, vsyncNs, intervalNs, latchNs));
            } finally {
                renderer.stop();
                codec.release();
            }
        }
    }

    private static MediaCodecDecoderRenderer renderer(Activity activity, boolean hostFrameTimes) {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
        prefs.upscalingMode = UpscalingPolicy.Mode.OFF;
        prefs.framePacing = PreferenceConfiguration.FRAME_PACING_MIN_LATENCY;
        prefs.hostFrameTimes = hostFrameTimes;
        MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity, prefs, null, 0, false, false, "",
                null, activity.getWindowManager().getDefaultDisplay());
        // The stream setup copies the preference.
        ReflectionHelpers.setField(renderer, "hostFrameTimes", hostFrameTimes);
        return renderer;
    }

    private static void samplePhase(MediaCodecDecoderRenderer renderer) {
        ReflectionHelpers.setField(renderer, "vsyncTimeNs", VSYNC_NS);
        ReflectionHelpers.setField(renderer, "vsyncIntervalNs", INTERVAL_NS);
        ReflectionHelpers.setField(renderer, "vsyncPresentationDeadlineNs", DEADLINE_NS);
    }

    private static void report(MediaCodecDecoderRenderer renderer, long readyNs) {
        ReflectionHelpers.callInstanceMethod(renderer, "reportPhaseSlack",
                ReflectionHelpers.ClassParameter.from(long.class, readyNs));
    }

    /** An instance whose constructor never ran (no GL thread): only its presence is read. */
    private static <T> T unconstructed(Class<T> type) throws Exception {
        Field theUnsafe = Class.forName("sun.misc.Unsafe").getDeclaredField("theUnsafe");
        theUnsafe.setAccessible(true);
        Object unsafe = theUnsafe.get(null);
        return type.cast(unsafe.getClass().getMethod("allocateInstance", Class.class).invoke(unsafe, type));
    }

    @Implements(value = MoonBridge.class, isInAndroidSdk = false)
    public static class SlackBridge {
        static final List<Long> slacks = new ArrayList<>();
        static final List<Long> intervals = new ArrayList<>();
        @Implementation protected static void __staticInitializer__() { }
        @Implementation protected static boolean reportFrameSlack(long slackNs, long intervalNs) {
            slacks.add(slackNs);
            intervals.add(intervalNs);
            return false;
        }
    }

    /** One decoded frame, then nothing: the renderer loop's next wait ends it. */
    @Implements(MediaCodec.class)
    public static class OneFrameCodec extends ShadowMediaCodec {
        static MediaCodecDecoderRenderer renderer;
        static FrameLatencyStats stats;
        static int dequeues, dropped;
        static long releasedAtNs, dueNs;

        @Implementation public int dequeueOutputBuffer(MediaCodec.BufferInfo info, long timeoutUs) {
            if (dequeues++ > 0) {
                // A poll for a newer output (no timeout) finds none; the loop's wait stops it.
                if (timeoutUs > 0) ReflectionHelpers.setField(renderer, "stopping", true);
                return MediaCodec.INFO_TRY_AGAIN_LATER;
            }
            long enqueue = System.nanoTime() - 1_000_000L;
            info.presentationTimeUs = 1;
            stats.onDecoderInput(1, 1, enqueue, enqueue, enqueue, (char) 0);
            return 0;
        }

        @Implementation public MediaFormat getOutputFormat() {
            return MediaFormat.createVideoFormat("video/avc", 1920, 1080);
        }

        @Implementation public void releaseOutputBuffer(int index, boolean render) {
            dropped++;
        }

        @Implementation public void releaseOutputBuffer(int index, long timestampNs) {
            releasedAtNs = System.nanoTime();
            dueNs = timestampNs;
        }
    }
}

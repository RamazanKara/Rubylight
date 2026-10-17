package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.media.MediaCodec;
import android.media.MediaFormat;

import com.limelight.preferences.PreferenceConfiguration;

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

import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.time.Duration;

import static com.limelight.preferences.PreferenceConfiguration.*;
import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, shadows = MediaCodecFramePacingTest.QueuedCodec.class)
public class MediaCodecFramePacingTest {
    @Test public void exactRateCanArriveBeforeSetupAndChangeWithinTheSameRoundedFps() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = renderer(activity.get(), FRAME_PACING_BALANCED);
            renderer.setStreamFrameRateMilliHz(59_940);
            assertEquals(59_940, renderer.getStreamFrameRateMilliHz());
            long generation = renderer.takeDecodeTiming(60).generation;
            MediaFormat format = MediaFormat.createVideoFormat("video/avc", 1920, 1080);
            ReflectionHelpers.setField(renderer, "configuredFormat", format);
            renderer.setStreamFrameRateMilliHz(59_950);
            assertEquals(59_950, renderer.getStreamFrameRateMilliHz());
            assertEquals(60, format.getInteger(MediaFormat.KEY_FRAME_RATE));
            assertEquals(generation + 1, renderer.takeDecodeTiming(60).generation);
            renderer.setStreamFrameRateMilliHz(59_950);
            assertEquals(generation + 1, renderer.takeDecodeTiming(60).generation);
            renderer.setFrameRate(60);
            assertEquals(60_000, renderer.getStreamFrameRateMilliHz());
            ReflectionHelpers.setField(renderer, "stopping", true);
            renderer.setStreamFrameRateMilliHz(119_880);
            assertEquals(60_000, renderer.getStreamFrameRateMilliHz());
        }
    }

    @Test public void smoothModesDropOnlyOldDecodedOutputsUntilTheBacklogIsCurrent() throws Exception {
        for (int mode : new int[] {FRAME_PACING_CAP_FPS, FRAME_PACING_MAX_SMOOTHNESS}) {
            Result result = runQueue(mode, new long[] {120, 100, 1, 0}, false, false);
            assertEquals(Arrays.asList(0, 1), result.dropped);
            assertEquals(Collections.singletonList(2), result.rendered);
            assertEquals(3, result.dequeues);
        }
    }

    @Test public void normalSmoothFramesArePresentedInOrderWithoutDraining() throws Exception {
        Result result = runQueue(FRAME_PACING_MAX_SMOOTHNESS, new long[] {1, 0}, false, false);
        assertTrue(result.dropped.isEmpty());
        assertEquals(Collections.singletonList(0), result.rendered);
        assertEquals(1, result.dequeues);
    }

    @Test public void oldLastFrameIsRetainedWhenNoReplacementExists() throws Exception {
        Result result = runQueue(FRAME_PACING_CAP_FPS, new long[] {120}, false, false);
        assertTrue(result.dropped.isEmpty());
        assertEquals(Collections.singletonList(0), result.rendered);
        assertEquals(2, result.dequeues);
    }

    @Test public void formatChangeDuringCatchupStillReachesTheRenderer() throws Exception {
        Result result = runQueue(FRAME_PACING_MAX_SMOOTHNESS, new long[] {120}, true, false);
        assertTrue(result.formatRead);
        assertTrue(result.dropped.isEmpty());
        assertEquals(Collections.singletonList(0), result.rendered);
    }

    @Test public void continuouslyStaleOutputCannotStarveStopAndRecoveryChecks() throws Exception {
        long[] ages = new long[100];
        Arrays.fill(ages, 120);
        Result result = runQueue(FRAME_PACING_MAX_SMOOTHNESS, ages, false, false);
        assertEquals(FramePacingPolicy.MAX_OUTPUT_DRAIN, result.dropped.size());
        assertEquals(Collections.singletonList(FramePacingPolicy.MAX_OUTPUT_DRAIN), result.rendered);
        assertEquals(FramePacingPolicy.MAX_OUTPUT_DRAIN + 1, result.dequeues);
    }

    @Test public void stopDuringCatchupLeavesNoDequeuedOutputUnreleased() throws Exception {
        Result result = runQueue(FRAME_PACING_MAX_SMOOTHNESS, new long[] {120, 100, 80}, false, true);
        assertEquals(Collections.singletonList(0), result.dropped);
        assertEquals(Collections.singletonList(1), result.rendered);
        assertEquals(2, result.dequeues);
    }

    private static MediaCodecDecoderRenderer renderer(Activity activity, int mode) {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
        prefs.framePacing = mode;
        return new MediaCodecDecoderRenderer(activity, prefs, null, 0, false, false, "", null);
    }

    private static Result runQueue(int mode, long[] agesMs, boolean formatChange, boolean stopOnDrop) throws Exception {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            ShadowSystemClock.advanceBy(Duration.ofSeconds(2));
            MediaCodecDecoderRenderer renderer = renderer(activity.get(), mode);
            renderer.setStreamFrameRateMilliHz(60_000);
            Result result = new Result();
            QueuedCodec.renderer = renderer;
            QueuedCodec.stats = ReflectionHelpers.getField(renderer, "frameLatencyStats");
            QueuedCodec.agesMs = agesMs;
            QueuedCodec.formatChange = formatChange;
            QueuedCodec.stopOnDrop = stopOnDrop;
            QueuedCodec.result = result;
            MediaCodec codec = MediaCodec.createDecoderByType("video/avc");
            try {
                ReflectionHelpers.setField(renderer, "videoDecoder", codec);
                ReflectionHelpers.callInstanceMethod(renderer, "startRendererThread");
                Thread thread = ReflectionHelpers.getField(renderer, "rendererThread");
                thread.join(5000);
                assertFalse("Renderer did not finish its output turn", thread.isAlive());
                return result;
            } finally {
                renderer.stop();
                codec.release();
            }
        }
    }

    private static final class Result {
        final List<Integer> dropped = new ArrayList<>();
        final List<Integer> rendered = new ArrayList<>();
        int dequeues;
        boolean formatRead;
    }

    @Implements(MediaCodec.class)
    public static class QueuedCodec extends ShadowMediaCodec {
        static MediaCodecDecoderRenderer renderer;
        static FrameLatencyStats stats;
        static long[] agesMs;
        static boolean formatChange;
        static boolean stopOnDrop;
        static Result result;

        @Implementation public int dequeueOutputBuffer(MediaCodec.BufferInfo info, long timeoutUs) {
            int index = result.dequeues++;
            if (index >= agesMs.length) {
                return formatChange ? MediaCodec.INFO_OUTPUT_FORMAT_CHANGED : MediaCodec.INFO_TRY_AGAIN_LATER;
            }
            long now = System.nanoTime();
            long enqueue = now - agesMs[index] * 1_000_000L;
            info.presentationTimeUs = index + 1;
            stats.onDecoderInput(index + 1, info.presentationTimeUs, enqueue, enqueue, enqueue, (char) 0);
            return index;
        }

        @Implementation public MediaFormat getOutputFormat() {
            result.formatRead = true;
            return MediaFormat.createVideoFormat("video/avc", 1920, 1080);
        }

        @Implementation public void releaseOutputBuffer(int index, boolean render) {
            assertFalse(render);
            result.dropped.add(index);
            if (stopOnDrop) ReflectionHelpers.setField(renderer, "stopping", true);
        }

        @Implementation public void releaseOutputBuffer(int index, long timestampNs) {
            result.rendered.add(index);
            ReflectionHelpers.setField(renderer, "stopping", true);
        }
    }
}

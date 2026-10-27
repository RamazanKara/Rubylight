package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.media.MediaCodec;
import android.os.SystemClock;

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

import java.time.Duration;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Set;

import static org.junit.Assert.*;

/** The production output loop runs against a codec with deterministic 120 FPS output arrivals. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, shadows = MediaCodecDrainBudgetTest.TimedCodec.class)
public class MediaCodecDrainBudgetTest {
    private static final long PERIOD_NS = 1_000_000_000L / 120;
    private static final int FRAMES = 360;

    @Test public void sustainableReleaseCostDoesNotDropOrdinaryFrames() throws Exception {
        Result result = run(8_000_000L, 0, 0);
        assertEquals(FRAMES, result.rendered.size());
        assertEquals(0, result.dropped);
        assertEquals(PERIOD_NS, result.maximumGap());
    }

    @Test public void finiteStallCannotKeepChasingFreshOutputsWithoutPresenting() throws Exception {
        long releaseCostNs = 8_000_000L; // Below the 8.333 ms period: steady delivery is sustainable.
        long pauseNs = 25_000_000L;
        Result result = run(releaseCostNs, pauseNs, 0);

        // A budget cannot interrupt one codec call. Allow one release before the pause and
        // one discard beyond the budget. Previously this pause caused 50 drops and a 433 ms gap.
        assertTrue(result.maximumGap() <= pauseNs + releaseCostNs + PERIOD_NS + releaseCostNs);
        assertTrue(result.maximumDrainRun <= 2);
        assertTrue("The finite backlog must clear", result.lastDroppedFrame < FRAMES - 120);
        assertTrue("Ready frames should be shown while catching up", result.rendered.size() > FRAMES - 50);
    }

    @Test public void continuouslyOverloadedCodecStillHandsOffEachBoundedCatchupTurn() throws Exception {
        long releaseCostNs = 9_000_000L; // Deliberate overload, distinct from the finite-stall case.
        Result result = run(releaseCostNs, 0, 0);
        assertTrue(result.dropped > 0);
        assertEquals(1, result.maximumDrainRun);
        assertTrue(result.maximumGap() <= PERIOD_NS + 2 * releaseCostNs);
        assertTrue(result.rendered.size() >= FRAMES / 2);
    }

    @Test public void fastCatchupStillPresentsTheNewestAlreadyAvailableOutput() throws Exception {
        Result result = run(250_000L, 0, 25_000_000L);
        assertEquals(3, result.firstRenderedFrame);
        assertEquals(3, result.dropped);
        assertEquals(3, result.maximumDrainRun);
    }

    private static Result run(long releaseCostNs, long pauseNs, long initialBacklogNs) throws Exception {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            ShadowSystemClock.advanceBy(Duration.ofSeconds(2));
            ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
            prefs.framePacing = PreferenceConfiguration.FRAME_PACING_MIN_LATENCY;
            MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity.get(), prefs,
                    null, 0, false, false, "", null);
            renderer.setStreamFrameRateMilliHz(120_000);
            Result result = new Result();
            TimedCodec.renderer = renderer;
            TimedCodec.stats = ReflectionHelpers.getField(renderer, "frameLatencyStats");
            TimedCodec.result = result;
            TimedCodec.nextFrame = 0;
            TimedCodec.originNs = SystemClock.elapsedRealtimeNanos() - initialBacklogNs;
            TimedCodec.releaseCostNs = releaseCostNs;
            TimedCodec.pauseNs = pauseNs;
            TimedCodec.pauseAtNs = TimedCodec.originNs + 1_000_000_000L;
            MediaCodec codec = MediaCodec.createDecoderByType("video/hevc");
            try {
                ReflectionHelpers.setField(renderer, "videoDecoder", codec);
                ReflectionHelpers.callInstanceMethod(renderer, "startRendererThread");
                Thread thread = ReflectionHelpers.getField(renderer, "rendererThread");
                thread.join(5000);
                assertFalse("Renderer did not finish", thread.isAlive());
                assertEquals(FRAMES, result.rendered.size() + result.dropped);
                assertTrue("Every dequeued output must be returned exactly once", result.owned.isEmpty());
                return result;
            } finally {
                renderer.stop();
                codec.release();
            }
        }
    }

    private static class Result {
        final List<Long> rendered = new ArrayList<>();
        final Set<Integer> owned = new HashSet<>();
        int dropped, drainRun, maximumDrainRun, lastDroppedFrame = -1, firstRenderedFrame = -1;

        long maximumGap() {
            long maximum = 0;
            for (int i = 1; i < rendered.size(); i++) {
                maximum = Math.max(maximum, rendered.get(i) - rendered.get(i - 1));
            }
            return maximum;
        }
    }

    @Implements(MediaCodec.class)
    public static class TimedCodec extends ShadowMediaCodec {
        static MediaCodecDecoderRenderer renderer;
        static FrameLatencyStats stats;
        static Result result;
        static int nextFrame;
        static long originNs, releaseCostNs, pauseNs, pauseAtNs;

        private static void advance(long nanos) {
            if (nanos > 0) ShadowSystemClock.advanceBy(Duration.ofNanos(nanos));
        }

        @Implementation public int dequeueOutputBuffer(MediaCodec.BufferInfo info, long timeoutUs) {
            if (nextFrame >= FRAMES) {
                ReflectionHelpers.setField(renderer, "stopping", true);
                return MediaCodec.INFO_TRY_AGAIN_LATER;
            }
            if (pauseNs > 0 && SystemClock.elapsedRealtimeNanos() >= pauseAtNs) {
                advance(pauseNs);
                pauseNs = 0;
            }
            long readyNs = originNs + nextFrame * PERIOD_NS;
            long nowNs = SystemClock.elapsedRealtimeNanos();
            if (nowNs < readyNs) {
                if (timeoutUs == 0) return MediaCodec.INFO_TRY_AGAIN_LATER;
                advance(readyNs - nowNs);
            }
            int frame = nextFrame++;
            assertTrue(result.owned.add(frame));
            info.presentationTimeUs = frame + 1;
            // Keep bookkeeping in its ordinary nanoTime domain. Only arrival timing and
            // codec call durations use the controlled Android elapsed clock.
            long inputNs = System.nanoTime() - 1_000_000L;
            stats.onDecoderInput(frame, info.presentationTimeUs, inputNs, inputNs, inputNs, (char) 0);
            return frame;
        }

        @Implementation public void releaseOutputBuffer(int index, boolean render) {
            assertFalse(render);
            assertTrue("Discard must own its buffer", result.owned.remove(index));
            result.dropped++;
            result.lastDroppedFrame = index;
            result.maximumDrainRun = Math.max(result.maximumDrainRun, ++result.drainRun);
            advance(releaseCostNs);
        }

        @Implementation public void releaseOutputBuffer(int index, long timestampNs) {
            assertTrue("Presentation must own its buffer", result.owned.remove(index));
            if (result.firstRenderedFrame < 0) result.firstRenderedFrame = index;
            result.rendered.add(SystemClock.elapsedRealtimeNanos());
            result.drainRun = 0;
            advance(releaseCostNs);
        }
    }
}

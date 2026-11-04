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
import org.robolectric.util.ReflectionHelpers;
import org.robolectric.util.ReflectionHelpers.ClassParameter;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

import static com.limelight.preferences.PreferenceConfiguration.FRAME_PACING_BALANCED;
import static com.limelight.preferences.PreferenceConfiguration.FRAME_PACING_CAP_FPS;
import static com.limelight.preferences.PreferenceConfiguration.FRAME_PACING_MAX_SMOOTHNESS;
import static com.limelight.preferences.PreferenceConfiguration.FRAME_PACING_MIN_LATENCY;
import static org.junit.Assert.*;

/** The renderer's side of DecoderQueuePolicy.Debt: releaseNewestOutput with a held output in debt. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, shadows = MediaCodecDebtReleaseTest.DebtCodec.class)
public class MediaCodecDebtReleaseTest {
    private static final int HELD = 20, NEWER = 21;
    private static final long MS = 1_000_000L;
    private static final long FRAME_NS = 1_000_000_000L / 60;

    @Test public void smoothModesDropTheHeldOutputForANewerOneThatCameOutWithinTheWait() {
        for (int mode : new int[] {FRAME_PACING_MAX_SMOOTHNESS, FRAME_PACING_CAP_FPS}) {
            Fixture fixture = new Fixture(mode, NEWER);
            try {
                fixture.release();
                assertEquals("mode " + mode, Collections.singletonList(DecoderQueuePolicy.Debt.WAIT_US), fixture.timeoutsUs);
                assertEquals("mode " + mode, Collections.singletonList(HELD), fixture.dropped);
                assertEquals("mode " + mode, Collections.singletonList(NEWER), fixture.rendered);
                assertEquals(2, ReflectionHelpers.<Integer>getField(fixture.renderer, "numFramesOut").intValue());
            } finally {
                fixture.close();
            }
        }
    }

    @Test public void withoutANewerOutputTheHeldOneIsShown() {
        Fixture fixture = new Fixture(FRAME_PACING_MAX_SMOOTHNESS, MediaCodec.INFO_TRY_AGAIN_LATER);
        try {
            fixture.release();
            assertEquals(Collections.singletonList(DecoderQueuePolicy.Debt.WAIT_US), fixture.timeoutsUs);
            assertTrue(fixture.dropped.isEmpty());
            assertEquals(Collections.singletonList(HELD), fixture.rendered);
            assertEquals(1, ReflectionHelpers.<Integer>getField(fixture.renderer, "numFramesOut").intValue());
        } finally {
            fixture.close();
        }
    }

    @Test public void withNothingBehindTheHeldOutputIsDroppedWithoutWaiting() {
        // The display's queue holds a buffer too many (issue 7, 10-12 ms for 45 s on a Shield):
        // no newer output will come, so the held one goes and the screen keeps the previous frame.
        for (int mode : new int[] {FRAME_PACING_MAX_SMOOTHNESS, FRAME_PACING_CAP_FPS}) {
            Fixture fixture = new Fixture(mode, MediaCodec.INFO_TRY_AGAIN_LATER, 0);
            try {
                fixture.release();
                assertTrue("mode " + mode + " has nothing to dequeue", fixture.timeoutsUs.isEmpty());
                assertEquals("mode " + mode, Collections.singletonList(HELD), fixture.dropped);
                assertTrue("mode " + mode, fixture.rendered.isEmpty());
                DecoderQueuePolicy.Debt debt = ReflectionHelpers.getField(fixture.renderer, "debt");
                assertFalse("one drop, then the next outputs are measured", debt.shouldDrop());
            } finally {
                fixture.close();
            }
        }
    }

    @Test public void latencyAndBalancedNeverWaitForDebt() {
        for (int mode : new int[] {FRAME_PACING_MIN_LATENCY, FRAME_PACING_BALANCED}) {
            Fixture fixture = new Fixture(mode, MediaCodec.INFO_TRY_AGAIN_LATER);
            try {
                fixture.release();
                assertEquals("mode " + mode + " only looks for a ready output", Collections.singletonList(0L), fixture.timeoutsUs);
                assertTrue(fixture.dropped.isEmpty());
                assertEquals(Collections.singletonList(HELD), fixture.rendered);
            } finally {
                fixture.close();
            }
        }
    }

    private static final class Fixture {
        final ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create();
        final MediaCodecDecoderRenderer renderer;
        final FrameLatencyStats stats;
        final MediaCodec codec;
        int next;
        final List<Long> timeoutsUs = new ArrayList<>();
        final List<Integer> dropped = new ArrayList<>();
        final List<Integer> rendered = new ArrayList<>();

        Fixture(int framePacing, int next) {
            this(framePacing, next, 1);
        }

        Fixture(int framePacing, int next, int behind) {
            ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
            prefs.framePacing = framePacing;
            renderer = new MediaCodecDecoderRenderer(activity.get(), prefs, null, 0, false, false, "", null);
            renderer.setStreamFrameRateMilliHz(60_000);
            ReflectionHelpers.setField(renderer, "outputVsyncAvailable", false);
            stats = ReflectionHelpers.getField(renderer, "frameLatencyStats");
            this.next = next;
            DebtCodec.fixture = this;
            try {
                codec = MediaCodec.createDecoderByType("video/hevc");
            } catch (java.io.IOException e) {
                throw new AssertionError(e);
            }
            ReflectionHelpers.setField(renderer, "videoDecoder", codec);

            // The held output, with behind frames queued behind it: 24 ms in the decoder against a
            // 1 ms floor, after 1.8 s of the same. No enqueue time, so the age rule stays out of it.
            long now = System.nanoTime();
            stats.onDecoderInput(HELD, HELD, now - 25 * MS, 0, now - 25 * MS, (char) 0);
            stats.onDecoderOutput(HELD, HELD, now - MS);
            ReflectionHelpers.setField(renderer, "numFramesIn", 1 + behind);
            ReflectionHelpers.setField(renderer, "numFramesOut", 1);
            ReflectionHelpers.setField(renderer, "decoderDepthBase", 0);
            DecoderQueuePolicy.Debt debt = ReflectionHelpers.getField(renderer, "debt");
            long startNs = now - 2_000_000_000L;
            debt.onOutput(MS, startNs, FRAME_NS, behind);
            for (int i = 1; i <= 110; i++) debt.onOutput(24 * MS, startNs + i * FRAME_NS, FRAME_NS, behind);
            assertTrue("primed into recovery", debt.shouldDrop());
        }

        void release() {
            ReflectionHelpers.callInstanceMethod(renderer, "releaseNewestOutput",
                    ClassParameter.from(int.class, HELD),
                    ClassParameter.from(long.class, (long) HELD),
                    ClassParameter.from(MediaCodec.BufferInfo.class, new MediaCodec.BufferInfo()),
                    ClassParameter.from(long.class, SystemClock.elapsedRealtimeNanos()));
        }

        void close() {
            ReflectionHelpers.setField(renderer, "stopping", true);
            codec.release();
            activity.close();
        }
    }

    @Implements(MediaCodec.class)
    public static class DebtCodec extends ShadowMediaCodec {
        static Fixture fixture;

        @Implementation public int dequeueOutputBuffer(MediaCodec.BufferInfo info, long timeoutUs) {
            fixture.timeoutsUs.add(timeoutUs);
            int result = fixture.next;
            fixture.next = MediaCodec.INFO_TRY_AGAIN_LATER;
            if (result == NEWER) {
                long now = System.nanoTime();
                info.presentationTimeUs = NEWER;
                fixture.stats.onDecoderInput(NEWER, NEWER, now - 9 * MS, 0, now - 9 * MS, (char) 0);
            }
            return result;
        }

        @Implementation public void releaseOutputBuffer(int index, boolean render) {
            assertFalse(render);
            fixture.dropped.add(index);
        }

        @Implementation public void releaseOutputBuffer(int index, long timestampNs) {
            fixture.rendered.add(index);
        }
    }
}

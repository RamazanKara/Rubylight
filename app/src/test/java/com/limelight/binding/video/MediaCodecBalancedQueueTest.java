package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.media.MediaCodec;
import android.view.Choreographer;

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

import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.LinkedBlockingQueue;
import java.util.concurrent.TimeUnit;

import static com.limelight.preferences.PreferenceConfiguration.FRAME_PACING_BALANCED;
import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, shadows = MediaCodecBalancedQueueTest.QueuedCodec.class)
public class MediaCodecBalancedQueueTest {
    private static final int OLD = 10, SECOND = 11, NEW = 12;
    private static final long FRAME_NS = 1_000_000_000L / 120;

    @Test public void replacementIsReadyWhileDiscardingTheOldOutputIsBlocked() throws Exception {
        try (Fixture fixture = new Fixture(1, new LinkedBlockingQueue<>())) {
            fixture.seed(OLD);
            fixture.start();
            assertTrue("Producer did not begin returning the old buffer", fixture.state.discardEntered.await(5, TimeUnit.SECONDS));

            // The real Choreographer branch runs while the codec's old-buffer release is paused.
            // Previously remove -> codec release -> add left this callback with an empty queue.
            fixture.frame(0);
            assertEquals(Collections.singletonList(NEW), fixture.state.rendered);
            assertEquals(Collections.singletonList(OLD), fixture.state.dropped);
            assertTrue(fixture.queue.isEmpty());
            assertFalse("Codec release held the handoff monitor", fixture.state.codecCallHeldQueueLock);
        }
    }

    @Test public void fullQueueDecisionCannotBecomeStaleAfterTheConsumerRemovesAFrame() throws Exception {
        SizeBarrierQueue queue = new SizeBarrierQueue();
        try (Fixture fixture = new Fixture(2, queue)) {
            fixture.seed(OLD);
            fixture.seed(SECOND);
            fixture.start();
            assertTrue("Producer did not inspect the full queue", queue.sizeRead.await(5, TimeUnit.SECONDS));
            try {
                fixture.frame(0);
            } finally {
                queue.consumerFinished.countDown();
            }
            assertTrue(fixture.state.discardEntered.await(5, TimeUnit.SECONDS));

            // The producer entered its atomic replacement first: it evicts OLD, then the
            // consumer gets SECOND and NEW remains. A stale size check instead consumes OLD
            // and needlessly evicts SECOND, even though the consumer already freed a slot.
            assertEquals(Collections.singletonList(OLD), fixture.state.dropped);
            assertEquals(Collections.singletonList(SECOND), fixture.state.rendered);
            assertEquals(Collections.singletonList(NEW), new ArrayList<>(fixture.queue));
        } finally {
            queue.consumerFinished.countDown();
        }
    }

    @Test public void consumerThatFreesASlotBeforeEnqueuePreservesBothRemainingFrames() throws Exception {
        try (Fixture fixture = new Fixture(2, new LinkedBlockingQueue<>())) {
            fixture.seed(OLD);
            fixture.seed(SECOND);
            fixture.frame(0);
            fixture.start();
            fixture.join();
            assertTrue(fixture.state.dropped.isEmpty());
            assertEquals(Arrays.asList(SECOND, NEW), new ArrayList<>(fixture.queue));

            // The fake decoder stopped after its single output; resume just the frame callbacks.
            ReflectionHelpers.setField(fixture.renderer, "stopping", false);
            fixture.frame(1);
            fixture.frame(2);
            assertEquals(Arrays.asList(OLD, SECOND, NEW), fixture.state.rendered);
            assertTrue(fixture.queue.isEmpty());
        }
    }

    /** Force the old size/poll race, but never wait for a consumer while holding its monitor. */
    private static final class SizeBarrierQueue extends LinkedBlockingQueue<Integer> {
        final CountDownLatch sizeRead = new CountDownLatch(1);
        final CountDownLatch consumerFinished = new CountDownLatch(1);
        boolean observed;

        @Override public int size() {
            int size = super.size();
            if (!observed && size == 2) {
                observed = true;
                sizeRead.countDown();
                if (!Thread.holdsLock(this)) await(consumerFinished);
            }
            return size;
        }
    }

    private static final class State {
        MediaCodecDecoderRenderer renderer;
        FrameLatencyStats stats;
        LinkedBlockingQueue<Integer> queue;
        final CountDownLatch discardEntered = new CountDownLatch(1);
        final CountDownLatch allowDiscard = new CountDownLatch(1);
        final List<Integer> dropped = new ArrayList<>();
        final List<Integer> rendered = new ArrayList<>();
        boolean outputDelivered, codecCallHeldQueueLock;
    }

    private static final class Fixture implements AutoCloseable {
        final ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create();
        final MediaCodecDecoderRenderer renderer;
        final LinkedBlockingQueue<Integer> queue;
        final State state = new State();
        final MediaCodec codec;
        final long firstVsync = System.nanoTime();

        Fixture(int capacity, LinkedBlockingQueue<Integer> queue) throws Exception {
            ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
            prefs.framePacing = FRAME_PACING_BALANCED;
            renderer = new MediaCodecDecoderRenderer(activity.get(), prefs, null, 0, false, false, "", null);
            renderer.setStreamFrameRateMilliHz(120_000);
            this.queue = queue;
            ReflectionHelpers.setField(renderer, "outputBufferQueue", queue);
            ReflectionHelpers.setField(renderer, "outputBufferQueueLimit", capacity);
            ReflectionHelpers.setField(renderer, "outputVsyncAvailable", true);
            ReflectionHelpers.setField(renderer, "vsyncIntervalNs", FRAME_NS);
            ReflectionHelpers.setField(renderer, "appVsyncOffsetNs", 0L);
            state.renderer = renderer;
            state.queue = queue;
            state.stats = ReflectionHelpers.getField(renderer, "frameLatencyStats");
            QueuedCodec.state = state;
            codec = MediaCodec.createDecoderByType("video/hevc");
            ReflectionHelpers.setField(renderer, "videoDecoder", codec);
        }

        void seed(int index) {
            long now = System.nanoTime();
            state.stats.onDecoderInput(index, index, now, now, now, (char) 0);
            state.stats.onDecoderOutput(index, index, now);
            queue.add(index);
        }

        void frame(int tick) {
            renderer.doFrame(firstVsync + tick * FRAME_NS);
        }

        void start() {
            ReflectionHelpers.callInstanceMethod(renderer, "startRendererThread");
        }

        void join() throws InterruptedException {
            Thread thread = ReflectionHelpers.getField(renderer, "rendererThread");
            if (thread != null) {
                thread.join(5000);
                assertFalse("Renderer thread did not finish", thread.isAlive());
            }
        }

        @Override public void close() throws Exception {
            state.allowDiscard.countDown();
            if (queue instanceof SizeBarrierQueue) ((SizeBarrierQueue) queue).consumerFinished.countDown();
            ReflectionHelpers.setField(renderer, "stopping", true);
            join();
            Choreographer.getInstance().removeFrameCallback(renderer);
            renderer.stop();
            codec.release();
            activity.close();
        }
    }

    private static void await(CountDownLatch latch) {
        try {
            if (!latch.await(5, TimeUnit.SECONDS)) throw new AssertionError("Timed out waiting for the controlled interleaving");
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new AssertionError(e);
        }
    }

    @Implements(MediaCodec.class)
    public static class QueuedCodec extends ShadowMediaCodec {
        static State state;

        @Implementation public int dequeueOutputBuffer(MediaCodec.BufferInfo info, long timeoutUs) {
            if (state.outputDelivered) {
                ReflectionHelpers.setField(state.renderer, "stopping", true);
                return MediaCodec.INFO_TRY_AGAIN_LATER;
            }
            state.outputDelivered = true;
            long now = System.nanoTime();
            info.presentationTimeUs = NEW;
            state.stats.onDecoderInput(NEW, NEW, now, now, now, (char) 0);
            return NEW;
        }

        @Implementation public void releaseOutputBuffer(int index, boolean render) {
            assertFalse(render);
            state.dropped.add(index);
            state.codecCallHeldQueueLock |= Thread.holdsLock(state.queue);
            state.discardEntered.countDown();
            await(state.allowDiscard);
        }

        @Implementation public void releaseOutputBuffer(int index, long timestampNs) {
            state.codecCallHeldQueueLock |= Thread.holdsLock(state.queue);
            state.rendered.add(index);
        }
    }
}

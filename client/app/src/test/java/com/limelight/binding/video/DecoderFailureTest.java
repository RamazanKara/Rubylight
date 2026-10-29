package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.graphics.SurfaceTexture;
import android.media.MediaCodec;
import android.view.Surface;

import com.limelight.PcViewLifecycleTest;
import com.limelight.nvstream.NvConnectionListener;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.After;
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

import java.lang.reflect.Proxy;
import java.time.Duration;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

/** A decoder that keeps failing ends the stream with its reason, never by crashing the app. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, instrumentedPackages = "com.limelight.nvstream.jni",
        shadows = {DecoderFailureTest.FailingCodec.class, PcViewLifecycleTest.NoNativeMoonBridge.class})
public class DecoderFailureTest {
    private static final String RELEASED = "Invalid to call at Released state; only valid in executing state";

    private final List<Integer> terminated = new CopyOnWriteArrayList<>();
    private final List<Throwable> uncaught = new CopyOnWriteArrayList<>();
    private final List<Exception> crashes = new CopyOnWriteArrayList<>();
    private Thread.UncaughtExceptionHandler previousHandler;
    private ActivityController<Activity> activity;
    private MediaCodecDecoderRenderer renderer;
    private MediaCodec codec;

    @Before
    public void setUp() throws Exception {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        previousHandler = Thread.getDefaultUncaughtExceptionHandler();
        Thread.setDefaultUncaughtExceptionHandler((thread, e) -> uncaught.add(e));
        NvConnectionListener listener = (NvConnectionListener) Proxy.newProxyInstance(
                NvConnectionListener.class.getClassLoader(), new Class<?>[] {NvConnectionListener.class},
                (proxy, method, args) -> {
                    if (method.getName().equals("connectionTerminated")) terminated.add((Integer) args[0]);
                    return null;
                });
        ReflectionHelpers.setStaticField(MoonBridge.class, "connectionListener", listener);

        activity = Robolectric.buildActivity(Activity.class).create();
        renderer = new MediaCodecDecoderRenderer(activity.get(), new PreferenceConfiguration(), crashes::add, 0,
                false, false, "", null);
        FailingCodec.released = true;
        codec = MediaCodec.createDecoderByType("video/hevc");
        ReflectionHelpers.setField(renderer, "videoDecoder", codec);
        ReflectionHelpers.setField(renderer, "activeDecoderName", "c2.mtk.hevc.decoder");
        ReflectionHelpers.setField(renderer, "initialWidth", 3840);
        ReflectionHelpers.setField(renderer, "initialHeight", 2160);
        // Each reset "works" and the codec fails again at once, as the TV's out of memory did.
        ReflectionHelpers.setField(renderer, "recoveryCodec", new NoOpRecovery());
        ReflectionHelpers.setField(renderer, "recoveryBackoff", new DecoderRecoveryBackoff(new long[] {0, 0, 0, 0, 0}));
    }

    @After
    public void tearDown() throws Exception {
        ReflectionHelpers.setField(renderer, "stopping", true);
        Thread thread = ReflectionHelpers.getField(renderer, "rendererThread");
        if (thread != null) thread.join(5000);
        ReflectionHelpers.setStaticField(MoonBridge.class, "connectionListener", null);
        Thread.setDefaultUncaughtExceptionHandler(previousHandler);
        FailingCodec.released = true;
        codec.release();
        activity.close();
    }

    @Test
    public void aReleasedCodecEndsTheStreamInsteadOfCrashingTheRendererThread() throws Exception {
        ReflectionHelpers.callInstanceMethod(renderer, "startRendererThread");
        Thread rendererThread = ReflectionHelpers.getField(renderer, "rendererThread");

        // The input thread: every dequeue throws, as in the TV's crash (fetchNextInputBuffer). It
        // runs beside the test, so a recovery that never ends fails the test instead of hanging it.
        List<Boolean> fetched = new CopyOnWriteArrayList<>();
        Thread input = new Thread(() -> {
            for (int i = 0; i < 200 && !(boolean) ReflectionHelpers.getField(renderer, "stopping"); i++) {
                fetched.add(ReflectionHelpers.callInstanceMethod(renderer, "fetchNextInputBuffer"));
            }
        });
        input.setDaemon(true);
        input.start();
        try {
            input.join(30_000);
            rendererThread.join(5000);
            assertFalse("Input thread still running", input.isAlive());
        } finally {
            renderer.prepareForStop();
        }

        assertFalse(fetched.toString(), fetched.contains(true));
        assertFalse("Renderer thread still running", rendererThread.isAlive());
        assertEquals(Collections.emptyList(), uncaught);
        assertEquals(Collections.singletonList(MoonBridge.CLIENT_ERROR_DECODER), terminated);
        String failure = renderer.getDecoderFailure();
        assertNotNull(failure);
        assertTrue(failure, failure.startsWith("c2.mtk.hevc.decoder failed again after 5 recoveries in a row" +
                " without a frame at 3840x2160 (out of memory"));
        assertFalse(renderer.decoderFailedWithVulkan());
        // Five recoveries, not ten in 2 s: the sixth gives up.
        assertEquals(5, (int) ReflectionHelpers.getField(renderer, "codecRecoveryAttempts"));
        // Reported once, so the next stream runs without reference frame invalidation.
        assertEquals(1, crashes.size());
        assertTrue(crashes.get(0) instanceof MediaCodecDecoderRenderer.RendererException);
    }

    @Test
    public void stoppingDuringABackOffWakesTheWaitingThreadsAtOnce() throws Exception {
        // The second wait is far longer than any stop may take, so the test holds on a slow runner:
        // with 250 ms, a busy CI machine saw the third recovery begin before it looked.
        DecoderRecoveryBackoff backoff = new DecoderRecoveryBackoff(new long[] {0, 60_000, 60_000, 60_000, 60_000});
        ReflectionHelpers.setField(renderer, "recoveryBackoff", backoff);
        ReflectionHelpers.callInstanceMethod(renderer, "startRendererThread");
        Thread rendererThread = ReflectionHelpers.getField(renderer, "rendererThread");
        Thread input = new Thread(() -> {
            while (!(boolean) ReflectionHelpers.getField(renderer, "stopping")) {
                ReflectionHelpers.callInstanceMethod(renderer, "fetchNextInputBuffer");
            }
        });
        // A failed assertion must not leave a thread that keeps the test JVM from exiting.
        input.setDaemon(true);
        input.start();
        long tookMs;
        try {
            // The second recovery in a row without a frame has begun its wait.
            long deadline = System.nanoTime() + 30_000_000_000L;
            while (backoff.recoveriesWithoutFrame() < 2 && System.nanoTime() < deadline) Thread.sleep(1);
            assertEquals(2, backoff.recoveriesWithoutFrame());

            // The stream ends from another thread, as the UI's stop does: the renderer thread is
            // interrupted and the input thread woken, instead of either sleeping the wait out.
            long stopNs = System.nanoTime();
            Thread stopper = new Thread(renderer::prepareForStop);
            stopper.start();
            stopper.join(5000);
            input.join(5000);
            rendererThread.join(5000);
            tookMs = (System.nanoTime() - stopNs) / 1_000_000;
        } finally {
            // Wakes both threads if an assertion above failed before the stop.
            renderer.prepareForStop();
            input.join(5000);
        }

        assertFalse("Input thread still running", input.isAlive());
        assertFalse("Renderer thread still running", rendererThread.isAlive());
        // Woken at once: well under the 60 s wait either thread was in.
        assertTrue("Stopping took " + tookMs + " ms", tookMs < 2000);
        assertEquals(Collections.emptyList(), uncaught);
        // A stop is not a decoder failure: no -107 and no reason.
        assertEquals(Collections.emptyList(), terminated);
        assertNull(renderer.getDecoderFailure());
        assertEquals(2, backoff.recoveriesWithoutFrame());
    }

    @Test
    public void aThreadWaitingForARecoveryLeavesOnceItIsDoneEvenIfTheDecoderFailedAgain() throws Exception {
        // The renderer thread has quiesced and waits for the input thread to recover the codec.
        failOnce();
        int renderThread = ReflectionHelpers.getStaticField(MediaCodecDecoderRenderer.class, "CR_FLAG_RENDER_THREAD");
        int inputThread = ReflectionHelpers.getStaticField(MediaCodecDecoderRenderer.class, "CR_FLAG_INPUT_THREAD");
        Thread waiting = new Thread(() -> ReflectionHelpers.callInstanceMethod(renderer, "doCodecRecoveryIfRequired",
                ReflectionHelpers.ClassParameter.from(int.class, renderThread)));
        waiting.setDaemon(true);
        waiting.start();
        Object monitor = ReflectionHelpers.getField(renderer, "codecRecoveryMonitor");
        long deadline = System.nanoTime() + 30_000_000_000L;
        while (waiting.getState() != Thread.State.TIMED_WAITING && System.nanoTime() < deadline) Thread.sleep(1);
        try {
            assertEquals(Thread.State.TIMED_WAITING, waiting.getState());
            // The input thread recovers, and its next dequeue throws before the waiting thread
            // looked again (a busy CPU): a new recovery is due. Holding the monitor makes that order
            // certain. The waiting thread kept waiting for this one, which needs it to quiesce
            // first, and the input thread waited for it: both decoder threads hung for good.
            synchronized (monitor) {
                assertTrue((boolean) ReflectionHelpers.callInstanceMethod(renderer, "doCodecRecoveryIfRequired",
                        ReflectionHelpers.ClassParameter.from(int.class, inputThread)));
                failOnce();
            }
            waiting.join(5000);
            assertFalse("The waiting thread never left its finished recovery", waiting.isAlive());
            assertEquals(1, (int) ReflectionHelpers.getField(renderer, "codecRecoveryAttempts"));

            // Both threads quiesce again, and the next recovery runs.
            recoverOnTheInputThread();
            assertEquals(2, (int) ReflectionHelpers.getField(renderer, "codecRecoveryAttempts"));
        } finally {
            renderer.prepareForStop();
            waiting.join(5000);
        }
    }

    @Test
    public void plannedRestartsNeitherWaitNorCountTowardGivingUp() {
        // Six planned restarts (HDR changes) with no frame between them: a healthy stream, no wait.
        DecoderRecoveryBackoff backoff = new DecoderRecoveryBackoff(new long[] {0, 60_000, 60_000, 60_000, 60_000});
        ReflectionHelpers.setField(renderer, "recoveryBackoff", backoff);
        for (int i = 0; i < 6; i++) {
            ReflectionHelpers.callInstanceMethod(renderer, "requestRestart",
                    ReflectionHelpers.ClassParameter.from(String.class, "HDR on"));
            recoverOnTheInputThread();
        }
        assertEquals(0, backoff.recoveriesWithoutFrame());
        assertEquals(6, (int) ReflectionHelpers.getField(renderer, "codecRecoveryAttempts"));
        assertTrue(terminated.isEmpty());
        assertNull(renderer.getDecoderFailure());
    }

    @Test
    public void aDecoderThatRefusesItsConfigurationOnAValidSurfaceEndsTheStream() {
        SurfaceTexture texture = new SurfaceTexture(0);
        Surface surface = new Surface(texture);
        try {
            ReflectionHelpers.setField(renderer, "renderTarget", surface);
            ReflectionHelpers.setField(renderer, "recoveryCodec", new RefusingRecovery());
            failOnce();
            recoverOnTheInputThread();
            assertEquals(Collections.singletonList(MoonBridge.CLIENT_ERROR_DECODER), terminated);
            assertEquals("c2.mtk.hevc.decoder refused its configuration after a recovery", renderer.getDecoderFailure());

            // With the surface gone, the same refusal only stops: the stream is ending anyway.
            terminated.clear();
            ReflectionHelpers.setField(renderer, "decoderFailure", null);
            ReflectionHelpers.setField(renderer, "stopping", false);
            surface.release();
            failOnce();
            recoverOnTheInputThread();
            assertTrue(terminated.isEmpty());
            assertTrue((boolean) ReflectionHelpers.getField(renderer, "stopping"));
        } finally {
            surface.release();
            texture.release();
        }
    }

    @Test
    public void anExceptionIsForgivenOnceFramesFlowAgain() {
        ReflectionHelpers.setField(renderer, "codecRecoveryAttempts", 10);
        failOnce();
        // Frames come out again: an exception a minute later is a new first one, not the end.
        ReflectionHelpers.setField(renderer, "numFramesOut", 30);
        ShadowSystemClock.advanceBy(Duration.ofMillis(60_000));
        failOnce();
        assertTrue(terminated.isEmpty());

        // And once frames flow after that one too, the hang check is back on.
        ReflectionHelpers.setField(renderer, "numFramesOut", 31);
        FailingCodec.released = false;
        assertFalse((boolean) ReflectionHelpers.callInstanceMethod(renderer, "fetchNextInputBuffer"));
        assertEquals(Collections.singletonList(MoonBridge.CLIENT_ERROR_DECODER), terminated);
        assertTrue(renderer.getDecoderFailure(),
                renderer.getDecoderFailure().startsWith("c2.mtk.hevc.decoder took no input for 5000 ms"));
    }

    @Test
    public void aDecoderThatStopsTakingInputAfterAnEatenExceptionStillEndsTheStream() {
        // One exception eaten (as at teardown), then no frame and no input without another throw:
        // the hang check stayed off for good.
        ReflectionHelpers.setField(renderer, "codecRecoveryAttempts", 10);
        failOnce();
        assertTrue(terminated.isEmpty());
        FailingCodec.released = false;
        assertFalse((boolean) ReflectionHelpers.callInstanceMethod(renderer, "fetchNextInputBuffer"));
        assertEquals(Collections.singletonList(MoonBridge.CLIENT_ERROR_DECODER), terminated);
        assertTrue(renderer.getDecoderFailure(),
                renderer.getDecoderFailure().startsWith("c2.mtk.hevc.decoder took no input for 5000 ms"));
    }

    private void failOnce() {
        ReflectionHelpers.callInstanceMethod(renderer, "handleDecoderException",
                ReflectionHelpers.ClassParameter.from(IllegalStateException.class, new IllegalStateException(RELEASED)));
    }

    // The renderer thread has quiesced; the input thread is the last and runs the recovery.
    private void recoverOnTheInputThread() {
        int renderThread = ReflectionHelpers.getStaticField(MediaCodecDecoderRenderer.class, "CR_FLAG_RENDER_THREAD");
        int inputThread = ReflectionHelpers.getStaticField(MediaCodecDecoderRenderer.class, "CR_FLAG_INPUT_THREAD");
        ReflectionHelpers.setField(renderer, "codecRecoveryThreadQuiescedFlags", renderThread);
        assertTrue((boolean) ReflectionHelpers.callInstanceMethod(renderer, "doCodecRecoveryIfRequired",
                ReflectionHelpers.ClassParameter.from(int.class, inputThread)));
    }

    @Test
    public void exceptionsAfterTheLastRecoveryEndTheStreamInsteadOfThrowing() {
        ReflectionHelpers.setField(renderer, "codecRecoveryAttempts", 10);
        IllegalStateException released = new IllegalStateException(RELEASED);
        ReflectionHelpers.callInstanceMethod(renderer, "handleDecoderException",
                ReflectionHelpers.ClassParameter.from(IllegalStateException.class, released));
        assertTrue(terminated.isEmpty());

        // Still failing 3 s later: this threw from the renderer thread and crashed the app.
        ShadowSystemClock.advanceBy(Duration.ofMillis(3000));
        ReflectionHelpers.callInstanceMethod(renderer, "handleDecoderException",
                ReflectionHelpers.ClassParameter.from(IllegalStateException.class, released));
        assertEquals(Collections.singletonList(MoonBridge.CLIENT_ERROR_DECODER), terminated);
        assertTrue(renderer.getDecoderFailure(), renderer.getDecoderFailure().contains(RELEASED));
        assertTrue((boolean) ReflectionHelpers.getField(renderer, "stopping"));

        // Later exceptions are eaten, and the stream is ended once.
        ReflectionHelpers.callInstanceMethod(renderer, "handleDecoderException",
                ReflectionHelpers.ClassParameter.from(IllegalStateException.class, released));
        assertEquals(1, terminated.size());
    }

    @Test
    public void aDecoderThatTakesNoInputEndsTheStream() {
        // The TV's HDR10 stream: 5000 ms dequeues, and the engine dropped the exception thrown for
        // it, so the picture stayed frozen.
        FailingCodec.released = false;
        assertFalse((boolean) ReflectionHelpers.callInstanceMethod(renderer, "fetchNextInputBuffer"));
        assertEquals(Collections.singletonList(MoonBridge.CLIENT_ERROR_DECODER), terminated);
        assertTrue(renderer.getDecoderFailure(),
                renderer.getDecoderFailure().startsWith("c2.mtk.hevc.decoder took no input for 5000 ms"));
    }

    private static final class NoOpRecovery implements CodecRecoverySteps.Codec {
        final List<String> steps = new ArrayList<>();

        @Override public void stop() { steps.add("stop"); }
        @Override public boolean recreate() { steps.add("recreate"); return true; }
        @Override public void reset() { steps.add("reset"); }
        @Override public void configure() { steps.add("configure"); }
        @Override public void rebuild(boolean codecFailed) { steps.add("rebuild"); }
    }

    /** Every configure fails as configure() does for a surface it cannot use. */
    private static final class RefusingRecovery implements CodecRecoverySteps.Codec {
        @Override public void stop() {}
        @Override public boolean recreate() { return true; }
        @Override public void reset() {}
        @Override public void configure() { throw new IllegalArgumentException("configure refused"); }
        @Override public void rebuild(boolean codecFailed) { throw new IllegalArgumentException("configure refused"); }
    }

    /** Released: every call throws, like a codec the platform let go. Otherwise it never has input. */
    @Implements(MediaCodec.class)
    public static class FailingCodec extends ShadowMediaCodec {
        static volatile boolean released;

        @Implementation public int dequeueInputBuffer(long timeoutUs) {
            if (released) throw new IllegalStateException(RELEASED);
            ShadowSystemClock.advanceBy(Duration.ofMillis(1000));
            return MediaCodec.INFO_TRY_AGAIN_LATER;
        }

        @Implementation public int dequeueOutputBuffer(MediaCodec.BufferInfo info, long timeoutUs) {
            if (released) throw new IllegalStateException(RELEASED);
            return MediaCodec.INFO_TRY_AGAIN_LATER;
        }
    }
}

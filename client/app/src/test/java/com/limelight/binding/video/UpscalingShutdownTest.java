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
import org.robolectric.shadow.api.Shadow;
import org.robolectric.util.ReflectionHelpers;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = {"com.limelight.nvstream.jni", "com.limelight.binding.video"})
public class UpscalingShutdownTest {
    private static GlesUpscaler allocateStubUpscaler(boolean frameWarp) {
        GlesUpscaler upscaler = Shadow.newInstanceOf(GlesUpscaler.class);
        ReflectionHelpers.setField(upscaler, "frameWarp", frameWarp);
        return upscaler;
    }

    @Test
    public void surfaceDestructionDoesNotWaitForGpuStartupOrRecoveryOnTheUiThread() throws Exception {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            // No codecs or GPU are needed to exercise the stop/recovery lock ordering.
            ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.upscalingMode = UpscalingPolicy.Mode.FSR1;
            prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
            MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity.get(), prefs,
                    null, 0, false, false, "", null);
            GlesUpscaler upscaler = allocateStubUpscaler(false);
            renderer.upscaler = upscaler;
            renderer.codecRecoveryType.set(MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_RESTART);
            CountDownLatch rendererInterrupted = new CountDownLatch(1);
            Thread fakeRenderer = new Thread(() -> {
                try {
                    Thread.sleep(5000);
                } catch (InterruptedException e) {
                    rendererInterrupted.countDown();
                }
            });
            fakeRenderer.start();
            renderer.rendererThread = fakeRenderer;
            Object monitor = ReflectionHelpers.getField(renderer, "codecRecoveryMonitor");
            CountDownLatch acquired = new CountDownLatch(1);
            CountDownLatch release = new CountDownLatch(1);
            AtomicBoolean timedOut = new AtomicBoolean();
            Thread recovery = new Thread(() -> {
                synchronized (monitor) {
                    acquired.countDown();
                    try {
                        timedOut.set(!release.await(5, TimeUnit.SECONDS));
                    } catch (InterruptedException e) {
                        Thread.currentThread().interrupt();
                    }
                }
            });
            recovery.start();
            try {
                assertTrue(acquired.await(5, TimeUnit.SECONDS));
                renderer.prepareForStop();
                assertTrue(ReflectionHelpers.<Boolean>getField(renderer, "stopping"));
                assertFalse("UI waited for the GPU recovery lock", timedOut.get());
                assertTrue("prepareForStop must pause GLES presentation synchronously",
                        ReflectionHelpers.<Boolean>getField(upscaler, "paused"));
                assertTrue("prepareForStop must interrupt rendererThread on UI thread even with upscaling",
                        rendererInterrupted.await(2, TimeUnit.SECONDS));
                assertEquals("prepareForStop must clear codecRecoveryType on UI thread",
                        MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_NONE, renderer.codecRecoveryType.get());
            } finally {
                renderer.upscaler = null;
                release.countDown();
                recovery.join(5000);
                fakeRenderer.interrupt();
                fakeRenderer.join(5000);
            }
            assertFalse(recovery.isAlive());
            Thread shutdown = new Thread(renderer::stop);
            shutdown.start();
            shutdown.join(5000);
            assertFalse("Shutdown worker did not finish", shutdown.isAlive());
        }
    }

    @Test
    public void cleanupSkipsClosingUpscalerWhenSubmitWorkerDidNotExit() throws Exception {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.upscalingMode = UpscalingPolicy.Mode.FSR1;
            MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity.get(), prefs,
                    null, 0, false, false, "", null);
            GlesUpscaler upscaler = allocateStubUpscaler(false);
            renderer.upscaler = upscaler;
            renderer.stopping = true;
            // A SubmitHandoff whose worker ignores interrupts so awaitSubmitWorker times out (workerEnded = false).
            CountDownLatch workerEntered = new CountDownLatch(1);
            CountDownLatch releaseWorker = new CountDownLatch(1);
            SubmitHandoff<MediaCodecDecodeLoop.HandedUnit> stuck = new SubmitHandoff<>(4, "TestStuckSubmit",
                    () -> {}, unit -> {
                        workerEntered.countDown();
                        while (releaseWorker.getCount() > 0) {
                            try {
                                releaseWorker.await(100, TimeUnit.MILLISECONDS);
                            } catch (InterruptedException ignored) {
                            }
                        }
                        return MoonBridge.DR_OK;
                    });
            stuck.enqueue(new MediaCodecDecodeLoop.HandedUnit(new byte[0], 0, 0, 0, 0, (char) 0, 0, 0, 0, 0), 1, true, 1);
            assertTrue(workerEntered.await(2, TimeUnit.SECONDS));
            renderer.submitHandoff = stuck;
            try {
                renderer.cleanup();
                assertSame("upscaler must stay open if submit worker is still alive", upscaler, renderer.upscaler);
                assertFalse("upscaler must not be closed while submit worker still holds decoder",
                        ReflectionHelpers.<Boolean>getField(upscaler, "closed"));
            } finally {
                releaseWorker.countDown();
                stuck.close(2000);
                renderer.upscaler = null;
            }
        }
    }

    @Test
    public void closeUpscalerDuringShutdownDoesNotDeclineFrameWarp() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.frameWarp = true;
            MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity.get(), prefs,
                    null, 0, false, false, "", null);
            renderer.upscalingPolicy = new UpscalingPolicy(60);
            renderer.vulkanHelper.frameWarp = true;
            GlesUpscaler upscaler = allocateStubUpscaler(true);
            android.os.HandlerThread fakeThread = new android.os.HandlerThread("StubUpscaler");
            fakeThread.start();
            ReflectionHelpers.setField(upscaler, "thread", fakeThread);
            ReflectionHelpers.setField(upscaler, "handler", new android.os.Handler(fakeThread.getLooper()));
            ReflectionHelpers.setField(upscaler, "display", android.opengl.EGL14.EGL_NO_DISPLAY);
            renderer.upscaler = upscaler;
            renderer.stopping = true;

            renderer.vulkanHelper.closeUpscaler();

            assertNull(renderer.upscaler);
            assertTrue(ReflectionHelpers.<Boolean>getField(upscaler, "closed"));
            assertTrue("shutdown closeUpscaler must not decline Frame Warp", renderer.vulkanHelper.frameWarp);
            assertFalse("shutdown closeUpscaler must not show Frame Warp fallback notice",
                    ReflectionHelpers.<Boolean>getField(renderer.vulkanHelper, "frameWarpNoticeShown"));
        }
    }
}

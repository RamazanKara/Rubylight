package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;

import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class UpscalingShutdownTest {
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
            } finally {
                release.countDown();
                recovery.join(5000);
            }
            assertFalse(recovery.isAlive());
            Thread shutdown = new Thread(renderer::stop);
            shutdown.start();
            shutdown.join(5000);
            assertFalse("Shutdown worker did not finish", shutdown.isAlive());
        }
    }
}

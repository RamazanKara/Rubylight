package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.hardware.display.DisplayManager;
import android.view.Display;

import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDisplayManager;
import org.robolectric.util.ReflectionHelpers;
import org.robolectric.util.ReflectionHelpers.ClassParameter;

import java.io.BufferedWriter;
import java.io.File;
import java.io.RandomAccessFile;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class MediaCodecDiagnosticsTest {
    @Test
    public void phaseTimingFollowsTheActiveDisplayModeAndDiscardsTheOldPhase() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.upscalingMode = UpscalingPolicy.Mode.OFF;
            prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
            MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity.get(), prefs,
                    null, 0, false, false, "", null);
            Display display = activity.get().getWindowManager().getDefaultDisplay();
            ReflectionHelpers.setField(renderer, "streamDisplay", display);
            int activeMode = display.getMode().getModeId();
            for (int hz : new int[] {120, 60, 90}) {
                Display.Mode mode = ReflectionHelpers.callConstructor(Display.Mode.class,
                        ClassParameter.from(int.class, activeMode), ClassParameter.from(int.class, 1080),
                        ClassParameter.from(int.class, 1920), ClassParameter.from(float.class, (float) hz));
                ShadowDisplayManager.setSupportedModes(display.getDisplayId(), mode);
                ReflectionHelpers.setField(renderer, "vsyncTimeNs", 1000000000L);
                DisplayManager.DisplayListener listener = ReflectionHelpers.getField(renderer, "displayListener");
                listener.onDisplayChanged(display.getDisplayId());
                assertEquals((long) (1000000000.0 / hz),
                        (long) ReflectionHelpers.getField(renderer, "vsyncIntervalNs"));
                assertEquals(0L, (long) ReflectionHelpers.getField(renderer, "vsyncTimeNs"));
            }
        }
    }

    @Test
    public void latencyCaptureRotatesOnlyWhenNewCompletedFramesAreAvailable() throws Exception {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.upscalingMode = UpscalingPolicy.Mode.OFF;
            prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
            MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity.get(), prefs,
                    null, 0, false, false, "", null);
            File capture = new File(activity.get().getFilesDir(), MediaCodecDecoderRenderer.LATENCY_CSV_NAME + ".tmp");
            ReflectionHelpers.callInstanceMethod(renderer, "openLatencyCsv");
            BufferedWriter writer = ReflectionHelpers.getField(renderer, "latencyCsv");
            writer.flush();
            try {
                try (RandomAccessFile file = new RandomAccessFile(capture, "rw")) {
                    file.setLength(8 * 1024 * 1024);
                }
                ReflectionHelpers.callInstanceMethod(renderer, "flushLatencyCsv");
                assertEquals(8 * 1024 * 1024, capture.length());

                FrameLatencyStats stats = ReflectionHelpers.getField(renderer, "frameLatencyStats");
                stats.onDecoderInput(1, 123, 1000000, 1500000, 2000000, (char) 0);
                stats.discard(123, "stream_ended");
                ReflectionHelpers.callInstanceMethod(renderer, "flushLatencyCsv");
                String csv = new String(Files.readAllBytes(capture.toPath()), StandardCharsets.UTF_8);
                assertTrue(csv.startsWith(FrameLatencyStats.CSV_HEADER));
                assertTrue(csv.contains(",stream_ended,"));
                assertTrue(capture.length() < 4096);
            } finally {
                ReflectionHelpers.callInstanceMethod(renderer, "closeLatencyCsv");
            }
        }
    }

    @Test
    public void hdrRestartsReachProblemReportsAsSuchNotAsCodecFailures() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
            MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity.get(), prefs,
                    null, 0, false, true, "", null);
            ReflectionHelpers.setField(renderer, "videoFormat", com.limelight.nvstream.jni.MoonBridge.VIDEO_FORMAT_H265_MAIN10);
            java.util.concurrent.atomic.AtomicInteger recovery = ReflectionHelpers.getField(renderer, "codecRecoveryType");
            byte[] metadata = new byte[26];
            // The PC's display goes from SDR to HDR during the 10-bit stream; the decoder restarts once
            // for it (metadata alone on a running HDR stream is a parameter instead).
            ReflectionHelpers.setField(renderer, "currentHdrMode", Boolean.FALSE);
            renderer.setHdrMode(true, metadata);
            assertEquals("restart", 2, recovery.get());
            assertTrue(com.limelight.LimeLog.getRedactedLog().contains("Decoder restart requested: HDR metadata"));
            recovery.set(0);
            renderer.setHdrMode(true, metadata);
            assertEquals("The same report again needs no restart", 0, recovery.get());
        }
    }
}

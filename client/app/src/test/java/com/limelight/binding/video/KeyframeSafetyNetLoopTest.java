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

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

/** The production output loop feeds the climb watch: decode latency comes from what the decoder hands back. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = {KeyframeSafetyNetLoopTest.OutputCodec.class, com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class KeyframeSafetyNetLoopTest {
    private static final long PERIOD_NS = 16_000_000L;
    // Nine seconds at 3 ms and three at 20 ms.
    private static final int CALM_FRAMES = 563;
    private static final int FRAMES = 750;

    /** Outputs on a 16 ms grid of the test's clock, each with the latency it should show. */
    @Implements(MediaCodec.class)
    public static class OutputCodec extends ShadowMediaCodec {
        static MediaCodecDecoderRenderer renderer;
        static FrameLatencyStats stats;
        static int nextFrame;
        static long originNs;

        @Implementation public int dequeueOutputBuffer(MediaCodec.BufferInfo info, long timeoutUs) {
            if (nextFrame >= FRAMES) {
                ReflectionHelpers.setField(renderer, "stopping", true);
                return MediaCodec.INFO_TRY_AGAIN_LATER;
            }
            long readyNs = originNs + nextFrame * PERIOD_NS;
            long nowNs = SystemClock.elapsedRealtimeNanos();
            if (nowNs < readyNs) {
                if (timeoutUs == 0) return MediaCodec.INFO_TRY_AGAIN_LATER;
                ShadowSystemClock.advanceBy(Duration.ofNanos(readyNs - nowNs));
            }
            int frame = nextFrame++;
            info.presentationTimeUs = frame + 1;
            // Bookkeeping stays in its ordinary nanoTime domain, as in MediaCodecDrainBudgetTest.
            long inputNs = System.nanoTime() - (frame < CALM_FRAMES ? 3 : 20) * 1_000_000L;
            stats.onDecoderInput(frame, info.presentationTimeUs, inputNs, inputNs, inputNs, (char) 0);
            return frame;
        }

        @Implementation public void releaseOutputBuffer(int index, boolean render) {}
        @Implementation public void releaseOutputBuffer(int index, long timestampNs) {}
    }

    @Test
    public void aClimbInTheDecodersOutputsIsSeen() throws Exception {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            ShadowSystemClock.advanceBy(Duration.ofSeconds(2));
            ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
            prefs.framePacing = PreferenceConfiguration.FRAME_PACING_MIN_LATENCY;
            MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity.get(), prefs,
                    null, 0, false, false, "", null);
            renderer.setStreamFrameRateMilliHz(60_000);
            KeyframeSafetyNet.Counts counts = new KeyframeSafetyNet.Counts();
            ReflectionHelpers.setField(renderer, "repairCounts", counts);
            ReflectionHelpers.setField(renderer, "keyframeSafetyNet", new KeyframeSafetyNet(counts));
            OutputCodec.renderer = renderer;
            OutputCodec.stats = ReflectionHelpers.getField(renderer, "frameLatencyStats");
            OutputCodec.nextFrame = 0;
            OutputCodec.originNs = SystemClock.elapsedRealtimeNanos();
            MediaCodec codec = MediaCodec.createDecoderByType("video/hevc");
            try {
                ReflectionHelpers.setField(renderer, "videoDecoder", codec);
                ReflectionHelpers.callInstanceMethod(renderer, "startRendererThread");
                Thread thread = ReflectionHelpers.getField(renderer, "rendererThread");
                thread.join(10_000);
                assertFalse("Renderer did not finish", thread.isAlive());
                assertEquals(counts.describe(), 1, counts.climbs);
                String log = com.limelight.LimeLog.getRedactedLog();
                assertTrue(log, log.contains("Decoder latency climb: "));
            } finally {
                renderer.stop();
                codec.release();
            }
        }
    }
}

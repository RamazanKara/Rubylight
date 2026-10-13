package com.limelight;

import android.app.Application;
import android.media.MediaFormat;

import com.limelight.binding.video.DisplayFrameRatePolicy;
import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.binding.video.MediaCodecHelper;
import com.limelight.nvstream.AdaptiveBitrateController;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, shadows = GameFrameRateTest.ReconfigureBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameFrameRateTest {
    @Test
    public void failedReconfigureKeepsFrameRateAndSuccessfulRetryUpdatesExistingPolicies() {
        Game activity = Robolectric.buildActivity(Game.class).get();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
        prefs.fps = 60;
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        ReconfigurableRenderer renderer = new ReconfigurableRenderer(activity, prefs);
        AdaptiveBitrateController bitrate = new AdaptiveBitrateController(20000, 16000, 60, 0);
        ReflectionHelpers.setField(activity, "prefConfig", prefs);
        ReflectionHelpers.setField(activity, "decoderRenderer", renderer);
        ReflectionHelpers.setField(activity, "adaptiveBitrate", bitrate);
        ReflectionHelpers.setField(activity, "connected", true);
        ReflectionHelpers.setField(activity, "fpsFollowsDisplay", true);
        ReflectionHelpers.setField(activity, "streamFps", 60);
        ReflectionHelpers.setField(activity, "streamPanelHz", 60f);
        ReflectionHelpers.setField(activity, "panelMaxRefreshRate", 120f);
        StreamReconfigureRequester requester = ReflectionHelpers.getField(activity, "reconfigureRequester");
        requester.start(1280, 720, 60000, 1280, 720, 60000);

        ReconfigureBridge.send = false;
        ReflectionHelpers.callInstanceMethod(activity, "checkStreamReconfigure");
        assertEquals(60, (int) ReflectionHelpers.getField(activity, "streamFps"));
        assertEquals(60, prefs.fps);
        assertEquals(60, (int) ReflectionHelpers.getField(renderer, "refreshRate"));
        assertEquals(60000, requester.fpsMillihz());
        assertEquals(0, bitrate.sample(16000, true, false, 0, 10));

        ReconfigureBridge.send = true;
        ReflectionHelpers.callInstanceMethod(activity, "checkStreamReconfigure");
        int streamFps = ReflectionHelpers.getField(activity, "streamFps");
        assertEquals(120, streamFps);
        assertEquals(120, prefs.fps);
        assertEquals(120, (int) ReflectionHelpers.getField(renderer, "refreshRate"));
        assertEquals(120000, requester.fpsMillihz());
        assertEquals(120, new DisplayFrameRatePolicy().cadenceVote(20, streamFps, 120, true, 0), 0);
        assertSame(bitrate, ReflectionHelpers.getField(activity, "adaptiveBitrate"));
        assertEquals(0, bitrate.sample(17000, true, false, 0, 10));
        assertEquals(12800, bitrate.sample(18000, true, false, 0, 10));
    }

    private static class ReconfigurableRenderer extends MediaCodecDecoderRenderer {
        ReconfigurableRenderer(Game activity, PreferenceConfiguration prefs) {
            super(activity, prefs, null, 0, false, false, "", null);
            ReflectionHelpers.setField(this, "refreshRate", prefs.fps);
            ReflectionHelpers.setField(this, "configuredFormat", MediaFormat.createVideoFormat("video/avc", 1280, 720));
        }

        @Override public int[] getReconfigureLimits() {
            return new int[] {4096, 4096, 2, 2};
        }

        @Override public boolean prepareForReconfigure(int width, int height) {
            return true;
        }
    }

    @Implements(value = MoonBridge.class, isInAndroidSdk = false)
    public static class ReconfigureBridge {
        static boolean send;

        @Implementation protected static void __staticInitializer__() { }

        @Implementation public static boolean hostSupportsControlMessage(int messageType) {
            return true;
        }

        @Implementation public static boolean requestReconfigure(int width, int height, int fpsMillihz) {
            return send;
        }
    }
}

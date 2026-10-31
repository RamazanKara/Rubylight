package com.limelight;

import android.app.Application;
import android.media.MediaFormat;

import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.binding.video.MediaCodecHelper;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.StreamOrientation;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.TimeUnit;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, shadows = GameRotateReconfigureTest.ReconfigureBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni", qualifiers = "w800dp-h400dp-land")
public class GameRotateReconfigureTest {
    @Before public void reset() {
        ReconfigureBridge.requests.clear();
    }

    private static Game connectedRotateStream(int launchWidth, int launchHeight) {
        Game activity = Robolectric.buildActivity(Game.class).get();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
        prefs.fps = 60;
        prefs.width = launchWidth;
        prefs.height = launchHeight;
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        ReflectionHelpers.setField(activity, "prefConfig", prefs);
        ReflectionHelpers.setField(activity, "decoderRenderer", new Renderer(activity, prefs, launchWidth, launchHeight));
        ReflectionHelpers.setField(activity, "streamOrientation", StreamOrientation.ROTATE);
        ReflectionHelpers.setField(activity, "hostShapesStream", true);
        ReflectionHelpers.setField(activity.videoOutput, "launchWidth", launchWidth);
        ReflectionHelpers.setField(activity.videoOutput, "launchHeight", launchHeight);
        ReflectionHelpers.setField(activity, "streamFps", 60);
        ReflectionHelpers.setField(activity, "connected", true);
        return activity;
    }

    @Test
    public void aTurnBeforeTheConnectionFinishedIsCorrectedOnceConnected() {
        // Launched in a portrait window; the phone turned to landscape while the stream connected.
        Game activity = connectedRotateStream(1080, 1920);
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "startReconfigureTracking");
        ShadowLooper.idleMainLooper(2, TimeUnit.SECONDS);
        assertEquals(1, ReconfigureBridge.requests.size());
        assertArrayEquals(new int[] {1920, 1080, 60000}, ReconfigureBridge.requests.get(0));

        // Once turned, nothing more is asked.
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "checkStreamReconfigure");
        assertEquals(1, ReconfigureBridge.requests.size());
    }

    @Test
    public void aStreamThatStartedInTheWindowsShapeAsksForNothing() {
        Game activity = connectedRotateStream(1920, 1080);
        ReflectionHelpers.callInstanceMethod(activity.videoOutput, "startReconfigureTracking");
        ShadowLooper.idleMainLooper(2, TimeUnit.SECONDS);
        assertTrue(ReconfigureBridge.requests.isEmpty());
    }

    private static class Renderer extends MediaCodecDecoderRenderer {
        private final int width, height;

        Renderer(Game activity, PreferenceConfiguration prefs, int width, int height) {
            super(activity, prefs, null, 0, false, false, "", null);
            this.width = width;
            this.height = height;
            ReflectionHelpers.setField(this, "configuredFormat", MediaFormat.createVideoFormat("video/avc", width, height));
        }

        @Override public int[] getStreamSize() {
            return new int[] {width, height};
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
        static final List<int[]> requests = new ArrayList<>();

        @Implementation protected static void __staticInitializer__() { }

        @Implementation public static boolean hostSupportsControlMessage(int messageType) {
            return true;
        }

        @Implementation public static boolean requestReconfigure(int width, int height, int fpsMillihz) {
            requests.add(new int[] {width, height, fpsMillihz});
            return true;
        }
    }
}

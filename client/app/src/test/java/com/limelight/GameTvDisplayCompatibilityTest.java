package com.limelight;

import android.app.Application;
import android.content.ComponentName;
import android.graphics.drawable.ColorDrawable;
import android.os.Looper;
import android.preference.PreferenceManager;
import android.view.View;
import android.view.ViewGroup;

import com.limelight.binding.input.ControllerHandler;
import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.binding.video.MediaCodecHelper;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.CompositorHeartbeat;
import com.limelight.ui.StreamView;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.util.concurrent.TimeUnit;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

/**
 * Settings › TV display compatibility in the stream: the dot runs in the window that holds the video while the
 * stream shows, read from the stored value (so a phone runs it too), and stops on pause, at the stream's end and
 * with the screen; picture-in-picture and the setting off have none.
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameTvDisplayCompatibilityTest {
    @After
    public void clear() {
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit().clear().commit();
    }

    /** A stream screen whose stream is connecting, with the setting stored as given; the start is the caller's. */
    private Game connecting(boolean on) {
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit()
                .putBoolean("checkbox_tv_display_compatibility", on).commit();
        Game game = Robolectric.buildActivity(Game.class).get();
        game.setTheme(R.style.StreamTheme);
        game.setContentView(R.layout.activity_game);
        StreamView stream = game.findViewById(R.id.surfaceView);
        game.streamView = stream;
        game.renderView = stream;
        game.prefConfig = PreferenceConfiguration.readPreferences(game);
        NvApp app = new NvApp("Desktop", 73, false);
        ReflectionHelpers.setField(game, "app", app);
        NvConnection conn = new NvConnection(null, new ComputerDetails.AddressTuple("192.0.2.1", 47989),
                47984, "test", new StreamConfiguration.Builder().setApp(app).build(), null, null) {
            @Override public void stop() {}
        };
        ReflectionHelpers.setField(game, "conn", conn);
        // The start announces the controllers again, of which this screen has none, and starts the
        // bitrate from the decoder's format. No Shield controller service answers here.
        shadowOf(game.getApplication()).declareComponentUnbindable(
                new ComponentName("com.nvidia.blakepairing", "com.nvidia.blakepairing.AccessoryService"));
        game.controllerHandler = new ControllerHandler(game, conn, game, game.prefConfig) {
            @Override public synchronized void resendControllerArrivals() {}
        };
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        game.decoderRenderer = new MediaCodecDecoderRenderer(game, game.prefConfig, null, 0, false, false, "", null);
        // As on a real start, the screen is resumed before the stream connects.
        game.foreground = true;
        game.videoOutput.updateCompositorHeartbeat();
        assertNull("nothing before the stream", game.videoOutput.compositorHeartbeat);
        return game;
    }

    /** The real start: GameConnection.connectionStarted, as the engine calls it. Whatever it throws fails the test. */
    private static void start(Game game) {
        game.connecting = true;
        game.streamConnection.connectionStarted();
        assertFalse("the start ran past its checks", game.connecting);
        shadowOf(Looper.getMainLooper()).idle();
    }

    private Game streaming(boolean on) {
        Game game = connecting(on);
        start(game);
        assertTrue(game.connected);
        return game;
    }

    private static void idle(long ms) {
        shadowOf(Looper.getMainLooper()).idleFor(ms, TimeUnit.MILLISECONDS);
    }

    private static int color(View dot) {
        return ((ColorDrawable) dot.getBackground()).getColor();
    }

    private static View dot(Game game) {
        CompositorHeartbeat heartbeat = game.videoOutput.compositorHeartbeat;
        return heartbeat == null ? null : heartbeat.dot();
    }

    @Test
    public void onTheDotShowsOverTheVideoAndAlternatesEvery100Ms() {
        Game game = streaming(true);
        View dot = dot(game);
        assertNotNull("running during the stream", dot);
        assertSame("in the window that holds the video", game.renderView.getParent(), dot.getParent());
        assertEquals(View.VISIBLE, dot.getVisibility());
        int first = color(dot);
        idle(CompositorHeartbeat.STREAM_INTERVAL_MS);
        int second = color(dot);
        assertNotEquals(first, second);
        idle(CompositorHeartbeat.STREAM_INTERVAL_MS);
        assertEquals(first, color(dot));
        assertFalse(dot.isFocusable());
        assertFalse(dot.isClickable());
        assertEquals(View.IMPORTANT_FOR_ACCESSIBILITY_NO, dot.getImportantForAccessibility());
    }

    @Test
    public void pauseStopsAndRemovesItAndResumeBringsItBack() {
        Game game = streaming(true);
        View dot = dot(game);
        game.onPause();
        assertNull(game.videoOutput.compositorHeartbeat);
        assertEquals(View.GONE, dot.getVisibility());
        shadowOf(Looper.getMainLooper()).idle();
        assertNull("gone on pause", dot.getParent());
        int paused = color(dot);
        idle(300);
        assertEquals("no tick while paused", paused, color(dot));
        game.onResume();
        assertNotNull("back with the stream", dot(game));
    }

    @Test
    public void inPictureInPictureThereIsNoDot() {
        Game game = connecting(true);
        game.enterPictureInPictureMode(new android.app.PictureInPictureParams.Builder().build());
        assertTrue(game.isInPictureInPictureMode());
        start(game);
        assertTrue(game.connected);
        assertNull("not while the stream is in picture-in-picture", game.videoOutput.compositorHeartbeat);
        game.videoOutput.updateCompositorHeartbeat();
        assertNull(game.videoOutput.compositorHeartbeat);
    }

    @Test
    public void offOnATvThereIsNoDot() {
        shadowOf(RuntimeEnvironment.getApplication().getPackageManager())
                .setSystemFeature(android.content.pm.PackageManager.FEATURE_LEANBACK, true);
        Game game = connecting(false);
        ViewGroup window = (ViewGroup) game.renderView.getParent();
        int children = window.getChildCount();
        start(game);
        assertTrue(game.connected);
        assertNull(game.videoOutput.compositorHeartbeat);
        idle(300);
        assertEquals(children, window.getChildCount());
    }

    @Test
    public void theStreamsEndStopsAndRemovesIt() {
        Game game = streaming(true);
        View dot = dot(game);
        ReflectionHelpers.callInstanceMethod(game.streamConnection, "stopConnection");
        assertNull(game.videoOutput.compositorHeartbeat);
        assertEquals(View.GONE, dot.getVisibility());
        shadowOf(Looper.getMainLooper()).idle();
        assertNull("gone at the stream's end", dot.getParent());
        int ended = color(dot);
        idle(300);
        assertEquals(ended, color(dot));
    }

    @Test
    public void nothingIsLeftWhenTheScreenGoes() {
        Game game = streaming(true);
        View dot = dot(game);
        game.onDestroy();
        assertNull(game.videoOutput.compositorHeartbeat);
        assertEquals(View.GONE, dot.getVisibility());
        shadowOf(Looper.getMainLooper()).idle();
        assertNull(dot.getParent());
    }

    @Test
    public void offThereIsNoDotAndNoTicking() {
        Game game = connecting(false);
        ViewGroup window = (ViewGroup) game.renderView.getParent();
        int children = window.getChildCount();
        start(game);
        assertNull(game.videoOutput.compositorHeartbeat);
        idle(300);
        assertEquals(children, window.getChildCount());
        assertNull(game.videoOutput.compositorHeartbeat);
    }

    @Test
    public void autoPulseShowsBrieflyAndAutoKeepaliveHoldsUntilStopped() {
        Game game = connecting(false);
        start(game);
        assertNull(game.videoOutput.compositorHeartbeat);

        game.videoOutput.pulseCompositorKeepalive(500L);
        shadowOf(Looper.getMainLooper()).idle();
        assertNotNull("500 ms pulse starts keepalive", dot(game));
        idle(500);
        assertNull("pulse clears after 500 ms", game.videoOutput.compositorHeartbeat);

        game.videoOutput.setAutoCompositorKeepalive(true);
        shadowOf(Looper.getMainLooper()).idle();
        assertNotNull("auto keepalive holds on", dot(game));
        idle(2000);
        assertNotNull("auto keepalive stays active beyond pulse duration", dot(game));

        game.videoOutput.setAutoCompositorKeepalive(false);
        shadowOf(Looper.getMainLooper()).idle();
        assertNull("auto keepalive stops cleanly", game.videoOutput.compositorHeartbeat);
    }

    @Test
    public void surfaceViewTargetWaitsForSurfaceCreatedAndStopsOnDestroy() {
        Game game = connecting(true);
        game.streamView.initializeSurface(false);
        assertFalse(game.surfaceCreated);
        start(game);
        assertNull("must wait until SurfaceView surfaceCreated before attaching keepalive",
                game.videoOutput.compositorHeartbeat);

        game.surfaceCreated = true;
        game.videoOutput.updateCompositorHeartbeat();
        shadowOf(Looper.getMainLooper()).idle();
        assertNotNull("keepalive starts once surfaceCreated is true", game.videoOutput.compositorHeartbeat);
        assertTrue(game.videoOutput.compositorHeartbeat.running());

        game.surfaceCreated = false;
        game.videoOutput.updateCompositorHeartbeat();
        assertNull("keepalive stops cleanly when SurfaceView surface destroys",
                game.videoOutput.compositorHeartbeat);
    }

    @Test
    public void textureViewWaitsForSurfaceCreatedAndPauseClearsUnlockedAutoKeepalive() {
        Game game = connecting(true);
        game.streamView.initializeSurface(true);
        start(game);
        assertNull("TextureView must wait until surfaceCreated is true",
                game.videoOutput.compositorHeartbeat);

        game.surfaceCreated = true;
        game.videoOutput.updateCompositorHeartbeat();
        shadowOf(Looper.getMainLooper()).idle();
        assertNotNull(game.videoOutput.compositorHeartbeat);

        game.surfaceCreated = false;
        game.videoOutput.updateCompositorHeartbeat();
        assertNull(game.videoOutput.compositorHeartbeat);

        game.prefConfig.tvDisplayCompatibility = false;
        game.surfaceCreated = true;
        game.videoOutput.setAutoCompositorKeepalive(true);
        shadowOf(Looper.getMainLooper()).idle();
        assertNotNull(game.videoOutput.compositorHeartbeat);

        game.onPause();
        assertNull("pause clears unlocked keepalive synchronously", game.videoOutput.compositorHeartbeat);
        game.onResume();
        assertNull("unlocked keepalive does not resume after pause", game.videoOutput.compositorHeartbeat);
    }
}

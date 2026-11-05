package com.limelight;

import android.app.Application;
import android.content.Context;
import android.content.Intent;

import com.limelight.console.OpenPlayModel;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.utils.LaunchSource;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.*;

/** Any way the player leaves an automatic start's stream clears the guard; a failure and its reconnect do not. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameOpenPlayLeaveTest {
    private Context context;

    @Before
    public void setUp() {
        context = RuntimeEnvironment.getApplication();
        // As the model answers: a stream the player left clears the guard, one that merely ended early does not.
        OpenPlayModel.use(request -> "{\"ok\":{\"disarm\":" + request.contains("\"left_by_player\":true") + "}}");
        armed(true);
    }

    @After
    public void tearDown() {
        OpenPlayModel.use(null);
        armed(false);
    }

    /** The guard's file as OpenPlay keeps it; its accessors belong to the console package. */
    private void armed(boolean armed) {
        context.getSharedPreferences("open_play", Context.MODE_PRIVATE).edit().putBoolean("armed", armed).commit();
    }

    private boolean armed() {
        return context.getSharedPreferences("open_play", Context.MODE_PRIVATE).getBoolean("armed", false);
    }

    private Game stream(String source) {
        Intent intent = new Intent(context, Game.class);
        LaunchSource.put(intent, source);
        Game game = Robolectric.buildActivity(Game.class, intent).get();
        ReflectionHelpers.setField(game, "prefConfig", new PreferenceConfiguration());
        return game;
    }

    /** The real screen, through its onCreate. */
    private Game created(String source) {
        Intent intent = new Intent(context, Game.class)
                .putExtra(Game.EXTRA_HOST, "192.0.2.1")
                .putExtra(Game.EXTRA_UNIQUEID, "device")
                .putExtra(Game.EXTRA_APP_ID, 11);
        LaunchSource.put(intent, source);
        Game game = Robolectric.buildActivity(Game.class, intent).create().get();
        assertNotNull("the screen got as far as its connection", game.conn);
        return game;
    }

    /** The configuration the real screen built for its connection. */
    private StreamConfiguration configOf(String source) {
        return ReflectionHelpers.getField(ReflectionHelpers.<Object>getField(created(source).conn, "context"), "streamConfig");
    }

    @Test
    public void aMissingDecoderIsAFailedStartWhenItsDialogClosesTheScreen() {
        Game game = created(LaunchSource.OPEN_PLAY);
        assertFalse("the case under test: no AVC decoder here", game.decoderRenderer.isAvcSupported());
        game.finish();
        assertTrue(armed());
    }

    @Test
    public void anAutomaticStartsConnectionMayNeverReplaceAGame() {
        assertTrue(configOf(LaunchSource.OPEN_PLAY).isNeverReplace());
    }

    @Test
    public void aStartThePlayerPressedMayReplaceAGameAsBefore() {
        assertFalse(configOf(LaunchSource.CONSOLE_PLAY).isNeverReplace());
    }

    @Test
    public void cancellingTheSpinnerOrAnyOtherFinishIsThePlayersChoice() {
        stream(LaunchSource.OPEN_PLAY).finish();
        assertFalse(armed());
    }

    @Test
    public void homeWithTheConnectionUnderWayIsThePlayersChoice() {
        stream(LaunchSource.OPEN_PLAY).onStop();
        assertFalse(armed());
    }

    @Test
    public void homeWithTheStreamKeptForTheReturnIsThePlayersChoice() {
        Game game = stream(LaunchSource.OPEN_PLAY);
        game.prefConfig.autoReconnect = true;
        game.stoppedStreamResumeIntent = new Intent();
        game.onStop();
        assertFalse("this path stays on the screen", game.isFinishing());
        assertFalse(armed());
    }

    @Test
    public void aStartThatFailedStaysArmedHoweverThePlayerThenLeaves() {
        Game game = stream(LaunchSource.OPEN_PLAY);
        game.streamConnection.stageFailed("RTSP", 0, 0);
        game.finish();
        assertTrue(armed());
    }

    @Test
    public void aStreamThatDroppedStaysArmedThroughItsReconnectScreen() {
        Game game = stream(LaunchSource.OPEN_PLAY);
        game.getIntent().putExtra(ReflectionHelpers.<String>getStaticField(GameRecovery.class, "EXTRA_RECOVERY_ATTEMPTS"), 1);
        game.finish();
        assertTrue("still connecting after a failure", armed());
    }

    @Test
    public void aHandOffToAnotherStreamScreenIsNotLeaving() {
        Game game = stream(LaunchSource.OPEN_PLAY);
        game.finishNow();
        game.onStop();
        assertTrue(armed());
    }

    @Test
    public void onlyAnAutomaticStartTouchesTheGuard() {
        stream(LaunchSource.CONSOLE_PLAY).finish();
        assertTrue(armed());
    }
}

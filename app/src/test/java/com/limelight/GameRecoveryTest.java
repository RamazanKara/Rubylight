package com.limelight;

import android.app.Application;
import android.os.Handler;
import android.os.Looper;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.preferences.PreferenceConfiguration;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;
import java.util.concurrent.atomic.AtomicInteger;
import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class GameRecoveryTest {
    private Game recoveryGame() {
        Game game = Robolectric.buildActivity(Game.class).get();
        game.setTheme(R.style.AppTheme);
        PreferenceConfiguration config = new PreferenceConfiguration();
        config.autoReconnect = true;
        ReflectionHelpers.setField(game, "prefConfig", config);
        ReflectionHelpers.setField(game, "app", new NvApp("Game", 73, false));
        ReflectionHelpers.setField(game, "foreground", true);
        ReflectionHelpers.setField(game, "connected", true);
        ReflectionHelpers.setField(game, "conn", new NvConnection(null,
                new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "test",
                new StreamConfiguration.Builder().build(), null, null) {
            @Override public void stop() {}
        });
        return game;
    }

    @Test public void recoveryQueuesOneAttemptAndStoppingCancelsIt() {
        Game game = recoveryGame();
        try {
            assertTrue(ReflectionHelpers.callInstanceMethod(game, "tryAutomaticRecovery"));
            assertNotNull(ReflectionHelpers.getField(game, "pendingRecovery"));
            assertFalse(ReflectionHelpers.callInstanceMethod(game, "tryAutomaticRecovery"));
            ReflectionHelpers.callInstanceMethod(game, "stopConnection");
            assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
            org.robolectric.Shadows.shadowOf(Looper.getMainLooper()).idleFor(java.time.Duration.ofSeconds(15));
            assertNull(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
        } finally {
            com.limelight.utils.SpinnerDialog.closeDialogs(game);
        }
    }

    @Test public void noRecoveryInBackgroundAfterQuitOrWhenDisabled() {
        Game game = recoveryGame();
        ReflectionHelpers.setField(game, "foreground", false);
        assertFalse(ReflectionHelpers.callInstanceMethod(game, "tryAutomaticRecovery"));
        ReflectionHelpers.setField(game, "foreground", true);
        ReflectionHelpers.setField(game, "connected", false);
        assertFalse(ReflectionHelpers.callInstanceMethod(game, "tryAutomaticRecovery"));
        ReflectionHelpers.setField(game, "connected", true);
        PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
        prefs.autoReconnect = false;
        assertFalse(ReflectionHelpers.callInstanceMethod(game, "tryAutomaticRecovery"));
    }

    @Test public void lifecycleCancellationRemovesRetryAndDeadlineCallbacks() {
        Game game = recoveryGame();
        AtomicInteger ran = new AtomicInteger();
        Runnable retry = ran::incrementAndGet;
        Handler handler = ReflectionHelpers.getField(game, "bitrateHandler");
        ReflectionHelpers.setField(game, "pendingRecovery", retry);
        handler.postDelayed(retry, 1000);
        ReflectionHelpers.callInstanceMethod(game, "cancelAutomaticRecovery");
        org.robolectric.Shadows.shadowOf(Looper.getMainLooper()).idleFor(java.time.Duration.ofSeconds(2));
        assertEquals(0, ran.get());
        assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
    }

    @Test public void cancellingWhileTeardownIsRunningCannotLaunchAnotherActivity() throws Exception {
        Game game = recoveryGame();
        java.util.concurrent.CountDownLatch stopEntered = new java.util.concurrent.CountDownLatch(1);
        java.util.concurrent.CountDownLatch stopReleased = new java.util.concurrent.CountDownLatch(1);
        ReflectionHelpers.setField(game, "conn", new NvConnection(null,
                new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "test",
                new StreamConfiguration.Builder().build(), null, null) {
            @Override public void stop() {
                stopEntered.countDown();
                try { stopReleased.await(2, java.util.concurrent.TimeUnit.SECONDS); }
                catch (InterruptedException interrupted) { Thread.currentThread().interrupt(); }
            }
        });
        try {
            assertTrue(ReflectionHelpers.callInstanceMethod(game, "tryAutomaticRecovery"));
            assertTrue(stopEntered.await(1, java.util.concurrent.TimeUnit.SECONDS));
            org.robolectric.Shadows.shadowOf(Looper.getMainLooper()).idleFor(java.time.Duration.ofSeconds(1));
            // The delayed retry is now waiting for teardown on a worker thread.
            assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
            ReflectionHelpers.callInstanceMethod(game, "stopConnection");
            stopReleased.countDown();
            Thread stopping = ReflectionHelpers.getField(game, "connectionStopThread");
            stopping.join(1000);
            // Any late posted continuation must observe the cancellation generation.
            for (int i = 0; i < 10; i++) {
                Thread.sleep(5);
                org.robolectric.Shadows.shadowOf(Looper.getMainLooper()).idle();
            }
            assertNull(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
        } finally {
            stopReleased.countDown();
            ReflectionHelpers.callInstanceMethod(game, "cancelAutomaticRecovery");
            com.limelight.utils.SpinnerDialog.closeDialogs(game);
        }
    }

    @Test public void pauseResumeDuringFirstRecoveryTeardownShowsManualRecoveryInsteadOfStrandedSpinner() throws Exception {
        assertPauseDuringHandoffShowsManualRecovery(true);
    }

    @Test public void automaticDisplayDetachUsesResumeOnlyEvenWhenNetworkRecoveryIsDisabled() throws Exception {
        Game game = recoveryGame();
        PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
        prefs.autoReconnect = false;
        ReflectionHelpers.setField(game, "recoveryAppId", 73);
        ReflectionHelpers.setField(game, "recoveryAppUuid", "captured-game");
        try {
            ReflectionHelpers.callInstanceMethod(game, "returnVideoToDevice");
            Thread stopping = ReflectionHelpers.getField(game, "connectionStopThread");
            stopping.join(1000);
            android.content.Intent retry = null;
            for (int i = 0; i < 20 && retry == null; i++) {
                Thread.sleep(5);
                org.robolectric.Shadows.shadowOf(Looper.getMainLooper()).idle();
                retry = org.robolectric.Shadows.shadowOf(game).getNextStartedActivity();
            }
            assertNotNull(retry);
            assertEquals(73, retry.getIntExtra(Game.EXTRA_APP_ID, 0));
            assertEquals("captured-game", retry.getStringExtra(Game.EXTRA_APP_UUID));
            assertFalse(retry.hasExtra("rubylight.outputDisplay"));
            Game replacement = Robolectric.buildActivity(Game.class).get();
            replacement.setIntent(retry);
            assertTrue(ReflectionHelpers.callInstanceMethod(replacement, "isResumeOnlyStream"));
            assertFalse(ReflectionHelpers.callInstanceMethod(replacement, "isRecoveryAttempt"));
        } finally {
            ReflectionHelpers.callInstanceMethod(game, "cancelAutomaticRecovery");
            com.limelight.utils.SpinnerDialog.closeDialogs(game);
        }
    }

    @Test public void pauseResumeDuringOutputReconnectShowsManualRecoveryInsteadOfStrandedSpinner() throws Exception {
        assertPauseDuringHandoffShowsManualRecovery(false);
    }

    private void assertPauseDuringHandoffShowsManualRecovery(boolean automatic) throws Exception {
        Game game = recoveryGame();
        java.util.concurrent.CountDownLatch stopEntered = new java.util.concurrent.CountDownLatch(1);
        java.util.concurrent.CountDownLatch stopReleased = new java.util.concurrent.CountDownLatch(1);
        ReflectionHelpers.setField(game, "conn", new NvConnection(null,
                new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "test",
                new StreamConfiguration.Builder().build(), null, null) {
            @Override public void stop() {
                stopEntered.countDown();
                try { stopReleased.await(10, java.util.concurrent.TimeUnit.SECONDS); }
                catch (InterruptedException interrupted) { Thread.currentThread().interrupt(); }
            }
        });
        try {
            if (automatic) assertTrue(ReflectionHelpers.callInstanceMethod(game, "tryAutomaticRecovery"));
            else ReflectionHelpers.callInstanceMethod(game, "reconnectStream");
            assertTrue(stopEntered.await(1, java.util.concurrent.TimeUnit.SECONDS));
            org.robolectric.Shadows.shadowOf(Looper.getMainLooper()).idleFor(java.time.Duration.ofSeconds(1));
            assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
            assertFalse(ReflectionHelpers.callInstanceMethod(game, "isRecoveryAttempt"));
            assertEquals(Boolean.TRUE, ReflectionHelpers.getField(game, "recoveryHandoffInProgress"));
            game.onPause();
            assertEquals(Boolean.TRUE, ReflectionHelpers.getField(game, "recoveryCancelledOnPause"));
            game.onResume();
            assertNull(ReflectionHelpers.getField(game, "spinner"));
            android.app.Dialog dialog = ReflectionHelpers.getField(game, "streamMenu");
            assertNotNull(dialog); assertTrue(dialog.isShowing());
            assertEquals(Boolean.FALSE, ReflectionHelpers.getField(game, "recoveryHandoffInProgress"));
            stopReleased.countDown();
            Thread stopping = ReflectionHelpers.getField(game, "connectionStopThread"); stopping.join(1000);
            for (int i = 0; i < 10; i++) {
                Thread.sleep(5); org.robolectric.Shadows.shadowOf(Looper.getMainLooper()).idle();
            }
            assertNull(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
        } finally {
            stopReleased.countDown();
            ReflectionHelpers.callInstanceMethod(game, "cancelAutomaticRecovery");
            android.app.Dialog dialog = ReflectionHelpers.getField(game, "streamMenu");
            if (dialog != null) dialog.dismiss();
            com.limelight.utils.SpinnerDialog.closeDialogs(game);
        }
    }
}

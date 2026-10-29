package com.limelight;

import android.app.Application;
import android.content.ComponentName;
import android.content.Intent;
import android.content.pm.ActivityInfo;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import com.limelight.binding.input.ControllerHandler;
import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.binding.video.MediaCodecHelper;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.util.ReflectionHelpers;
import java.util.concurrent.atomic.AtomicInteger;
import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class GameRecoveryTest {
    private static final String MICROPHONE_MUTED = "rubylight.microphoneMuted";
    /** How long a background step may take before the test calls it hung; a loaded machine stays well inside it. */
    private static final long HANG_GUARD_MS = 10_000;
    /** Game's workers that wait for the stream's stop, then post the hand-off to the UI thread. */
    private static final java.util.Set<String> HANDOFF_WORKERS =
            java.util.Set.of("Stream reconnect", "Automatic stream recovery");

    @Test public void mergedManifestAllowsReturningToAStoppedStream() throws Exception {
        Game game = Robolectric.buildActivity(Game.class).get();
        ActivityInfo info = game.getPackageManager().getActivityInfo(new ComponentName(game, Game.class), 0);
        assertEquals(0, info.flags & ActivityInfo.FLAG_NO_HISTORY);
    }

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

    private Game establishedGame() {
        Game game = recoveryGame();
        PreferenceConfiguration config = ReflectionHelpers.getField(game, "prefConfig");
        config.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
        config.enableLatencyToast = false;
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        ReflectionHelpers.setField(game, "decoderRenderer", new MediaCodecDecoderRenderer(game, config,
                null, 0, false, false, "", null) {
            @Override public void prepareForStop() {}
        });
        ReflectionHelpers.setField(game, "tombstonePrefs", game.getSharedPreferences("recovery-test", 0));
        ReflectionHelpers.setField(game, "attemptedConnection", true);
        ReflectionHelpers.setField(game, "recoveryAppId", 73);
        ReflectionHelpers.setField(game, "recoveryAppUuid", "captured-game");
        ReflectionHelpers.setField(game, "connectedAt", SystemClock.elapsedRealtime());
        return game;
    }

    @Test public void automaticRecoveryRestoresLocalAndControllerHostMuteBeforeCapture() throws Exception {
        for (boolean controllerMuted : new boolean[] {false, true}) {
            Game game = recoveryGame();
            MuteConnection connection = new MuteConnection();
            ReflectionHelpers.setField(game, "conn", connection);
            if (controllerMuted) connection.controllerMicrophoneState(new Object(), false, false, true);
            else connection.setMicrophoneMuted(true);
            try {
                assertTrue(ReflectionHelpers.callInstanceMethod(game, "tryAutomaticRecovery"));
                Intent retry = awaitRecoveryLaunch(game, 1);
                assertRestoredMute(retry, true);
            } finally {
                cleanupRecovery(game);
            }
        }
    }

    @Test public void lockResumeKeepsHostMuteAfterTheStoppedConnectionClearsControllerState() throws Exception {
        Game game = establishedGame();
        MuteConnection connection = new MuteConnection();
        connection.controllerMicrophoneState(new Object(), false, false, true);
        ReflectionHelpers.setField(game, "conn", connection);
        try {
            game.onPause();
            game.onStop();
            Thread stopping = ReflectionHelpers.getField(game, "connectionStopThread");
            join(stopping);
            assertFalse(connection.isMicrophoneMutedForReconnect());
            game.onResume();
            assertRestoredMute(awaitRecoveryLaunch(game, 1), true);
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test public void manualAndOutputReconnectPreserveTheCurrentMuteWithoutPersistingAnOldMute() {
        for (boolean outputSwitch : new boolean[] {false, true}) {
            Game game = recoveryGame();
            MuteConnection connection = new MuteConnection();
            ReflectionHelpers.setField(game, "conn", connection);
            ReflectionHelpers.setField(game, "outputSwitchPending", outputSwitch);
            connection.setMicrophoneMuted(true);
            assertRestoredMute(ReflectionHelpers.callInstanceMethod(game, "createReconnectIntent"), true);
            connection.setMicrophoneMuted(false);
            assertRestoredMute(ReflectionHelpers.callInstanceMethod(game, "createReconnectIntent"), false);
        }
        // A separate fresh launch still uses the unmuted default; the choice belongs to this session.
        assertRestoredMute(new Intent(), false);
    }

    @Test public void manualReconnectKeepsTheMuteSnapshotWhenTheStoppedConnectionIsGone() throws Exception {
        Game game = recoveryGame();
        MuteConnection connection = new MuteConnection();
        connection.controllerMicrophoneState(new Object(), false, false, true);
        ReflectionHelpers.setField(game, "conn", connection);
        ReflectionHelpers.callInstanceMethod(game, "stopConnection");
        Thread stopping = ReflectionHelpers.getField(game, "connectionStopThread");
        join(stopping);
        assertFalse(connection.isMicrophoneMutedForReconnect());
        ReflectionHelpers.setField(game, "conn", null);
        assertRestoredMute(ReflectionHelpers.callInstanceMethod(game, "createReconnectIntent"), true);
    }

    @Test
    @Config(shadows = PcViewLifecycleTest.NoNativeMoonBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void finalControllerTeardownMuteReachesAlreadyPreparedRecoveryAndReconnectIntents() throws Exception {
        for (boolean automatic : new boolean[] {false, true}) {
            Game game = recoveryGame();
            MuteConnection connection = new MuteConnection();
            ReflectionHelpers.setField(game, "conn", connection);
            PreferenceConfiguration config = ReflectionHelpers.getField(game, "prefConfig");
            // This fixture has no Shield service; do not post a successful binding with a null binder.
            org.robolectric.Shadows.shadowOf(game.getApplication()).declareComponentUnbindable(
                    new ComponentName("com.nvidia.blakepairing", "com.nvidia.blakepairing.AccessoryService"));
            ControllerHandler handler = new ControllerHandler(game, connection, null, config) {
                @Override public synchronized void stop() {
                    // A USB worker can publish its last host mute while being synchronously joined.
                    Object owner = new Object();
                    connection.controllerMicrophoneState(owner, false, false, true);
                    connection.releaseControllerMicrophone(owner);
                    super.stop();
                }
            };
            ReflectionHelpers.setField(game, "controllerHandler", handler);
            try {
                assertFalse(connection.isMicrophoneMutedForReconnect());
                if (automatic) assertTrue(ReflectionHelpers.callInstanceMethod(game, "tryAutomaticRecovery"));
                else ReflectionHelpers.callInstanceMethod(game, "reconnectStream");
                assertRestoredMute(awaitRecoveryLaunch(game, automatic ? 1 : 0), true);
            } finally {
                cleanupRecovery(game);
                handler.destroy();
            }
        }
    }

    private static void assertRestoredMute(Intent retry, boolean muted) {
        assertNotNull(retry);
        assertEquals(muted, retry.getBooleanExtra(MICROPHONE_MUTED, false));
        Game replacement = Robolectric.buildActivity(Game.class).get();
        replacement.setIntent(retry);
        MuteConnection connection = new MuteConnection();
        ReflectionHelpers.setField(replacement, "conn", connection);
        ReflectionHelpers.callInstanceMethod(replacement, "restoreMicrophoneMute");
        assertEquals(muted, connection.isMicrophoneMuted());
        assertEquals(muted, connection.isMicrophoneMutedForReconnect());
    }

    private static class MuteConnection extends NvConnection {
        MuteConnection() {
            super(null, new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().setApp(new NvApp("Game", 73, false)).build(), null, null);
        }

        @Override public void stop() {
            // Match native teardown's state removal without opening a socket, device, or microphone.
            java.util.Map<?, ?> microphones = ReflectionHelpers.getField(this, "controllerMicrophones");
            microphones.clear();
        }

        @Override public void sendControllerInput(short number, short mask, int buttons, byte leftTrigger,
                                                   byte rightTrigger, short leftX, short leftY, short rightX, short rightY) { }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void nativeStartupTimeoutQueuesNextRecoveryWithoutAConnectivityProbe() {
        Game game = recoveryGame();
        long started = SystemClock.elapsedRealtime();
        game.setIntent(new Intent().putExtra("RecoveryAttempts", 1).putExtra("RecoveryStarted", started));
        StreamRecoveryPolicy policy = new StreamRecoveryPolicy(1, started);
        ReflectionHelpers.setField(game, "recoveryPolicy", policy);
        ReflectionHelpers.setField(game, "connected", false);
        ReflectionHelpers.setField(game, "connecting", true);
        RecoveryBridge.connectivityTests = 0;
        try {
            game.stageFailed("Control stream establishment", MoonBridge.ML_PORT_FLAG_UDP_47999, 110);
            assertNotNull(ReflectionHelpers.getField(game, "pendingRecovery"));
            assertEquals(2, policy.attempts());
            assertEquals(0, RecoveryBridge.connectivityTests);
            assertNull(ReflectionHelpers.getField(game, "streamMenu"));
        } finally {
            cleanupRecovery(game);
        }
    }

    @Implements(value = MoonBridge.class, isInAndroidSdk = false)
    public static class RecoveryBridge {
        static int connectivityTests;
        @Implementation protected static void __staticInitializer__() {}
        @Implementation public static int testClientConnectivity(String server, int port, int portFlags) {
            connectivityTests++;
            return 0;
        }
        @Implementation public static String stringifyPortFlags(int portFlags, String separator) {
            return Integer.toString(portFlags);
        }
        @Implementation public static int getPortFlagsFromTerminationErrorCode(int errorCode) { return 0; }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void confirmedHostExitFinishesWithoutAnErrorOrRetryWithRecoveryOnOrOff() {
        for (boolean recoveryEnabled : new boolean[] {false, true}) {
            Game game = establishedGame();
            PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
            prefs.autoReconnect = recoveryEnabled;
            AtomicInteger probes = new AtomicInteger();
            ReflectionHelpers.setField(game, "conn", new MuteConnection() {
                @Override public boolean hasEndedSession() { probes.incrementAndGet(); return true; }
            });
            RecoveryBridge.connectivityTests = 0;
            try {
                game.connectionTerminated(-1);
                assertEquals(1, probes.get());
                assertTrue(game.isFinishing());
                assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
                assertNull(ReflectionHelpers.getField(game, "streamMenu"));
                assertEquals(0, RecoveryBridge.connectivityTests);
                assertNull(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
            } finally {
                cleanupRecovery(game);
            }
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void unconfirmedDisconnectStillRetriesOrShowsTheOriginalErrorAndKeepsItsReason() {
        for (boolean recoveryEnabled : new boolean[] {false, true}) {
            Game game = establishedGame();
            PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
            prefs.autoReconnect = recoveryEnabled;
            ReflectionHelpers.setField(game, "conn", new MuteConnection() {
                @Override public boolean hasEndedSession() { return false; }
            });
            try {
                game.connectionTerminated(-1);
                // With recovery off the stream closes and says why in a message that outlives it.
                assertEquals(!recoveryEnabled, game.isFinishing());
                assertEquals(recoveryEnabled, ReflectionHelpers.getField(game, "pendingRecovery") != null);
                assertNull(ReflectionHelpers.getField(game, "streamMenu"));
                if (!recoveryEnabled) {
                    String toast = org.robolectric.shadows.ShadowToast.getTextOfLatestToast();
                    assertEquals("Connection Terminated\nError code: -1", toast);
                }
                assertTrue(LimeLog.getRedactedLog().contains("Connection terminated: -1"));
            } finally {
                cleanupRecovery(game);
            }
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void quittingFromTheStreamMenuEndsQuietlyWhenTheHostDropsTheStream() throws Exception {
        for (boolean recoveryEnabled : new boolean[] {false, true}) {
            Game game = establishedGame();
            PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
            prefs.autoReconnect = recoveryEnabled;
            java.util.concurrent.CountDownLatch quitting = new java.util.concurrent.CountDownLatch(1);
            java.util.concurrent.CountDownLatch answered = new java.util.concurrent.CountDownLatch(1);
            ReflectionHelpers.setField(game, "conn", new MuteConnection() {
                @Override public void quitApp() {
                    quitting.countDown();
                    // The host tears the stream down before /cancel answers.
                    try { answered.await(HANG_GUARD_MS, java.util.concurrent.TimeUnit.MILLISECONDS); }
                    catch (InterruptedException e) { Thread.currentThread().interrupt(); }
                }
                // The game is still exiting, so the host does not look idle yet.
                @Override public boolean hasEndedSession() { return false; }
            });
            RecoveryBridge.connectivityTests = 0;
            try {
                ReflectionHelpers.callInstanceMethod(game, "quitStreamApp");
                assertTrue(quitting.await(HANG_GUARD_MS, java.util.concurrent.TimeUnit.MILLISECONDS));
                game.connectionTerminated(-1);
                assertTrue(game.isFinishing());
                assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
                assertNull(ReflectionHelpers.getField(game, "streamMenu"));
                assertEquals(0, RecoveryBridge.connectivityTests);
                assertNull(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
            } finally {
                answered.countDown();
                cleanupRecovery(game);
            }
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aFailedQuitRequestLeavesLaterFailuresVisible() throws Exception {
        Game game = establishedGame();
        PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
        prefs.autoReconnect = false;
        java.util.concurrent.CountDownLatch refused = new java.util.concurrent.CountDownLatch(1);
        ReflectionHelpers.setField(game, "conn", new MuteConnection() {
            @Override public void quitApp() throws java.io.IOException {
                refused.countDown();
                throw new java.io.IOException("The host did not quit the app.");
            }
            @Override public boolean hasEndedSession() { return false; }
        });
        try {
            ReflectionHelpers.callInstanceMethod(game, "quitStreamApp");
            assertTrue(refused.await(HANG_GUARD_MS, java.util.concurrent.TimeUnit.MILLISECONDS));
            awaitHostAction(game);
            // The stream outlived the time a dropped stream takes to report.
            org.robolectric.shadows.ShadowLooper.idleMainLooper(16, java.util.concurrent.TimeUnit.SECONDS);
            game.connectionTerminated(-1);
            // Not a graceful exit: the reason shows in a message that outlives the stream.
            assertTrue(game.isFinishing());
            String toast = org.robolectric.shadows.ShadowToast.getTextOfLatestToast();
            assertTrue(toast, toast.equals("Connection Terminated\nError code: -1"));
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aRequestThatFailsAfterTheHostDroppedTheStreamStillEndsQuietly() throws Exception {
        for (String request : new String[] {"quitStreamApp", "endRemoteSession"}) {
            for (boolean recoveryEnabled : new boolean[] {false, true}) {
                Game game = establishedGame();
                PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
                prefs.autoReconnect = recoveryEnabled;
                java.util.concurrent.CountDownLatch failed = new java.util.concurrent.CountDownLatch(1);
                ReflectionHelpers.setField(game, "conn", new MuteConnection() {
                    // The host ended the session and dropped the stream, then the reply was lost.
                    @Override public void quitApp() throws java.io.IOException {
                        failed.countDown();
                        throw new java.net.SocketException("Connection reset");
                    }
                    @Override public void disconnectRemoteSession() throws java.io.IOException {
                        failed.countDown();
                        throw new java.net.SocketException("Connection reset");
                    }
                    // The game is still exiting, so the host does not look idle yet.
                    @Override public boolean hasEndedSession() { return false; }
                });
                RecoveryBridge.connectivityTests = 0;
                try {
                    ReflectionHelpers.callInstanceMethod(game, request);
                    assertTrue(failed.await(HANG_GUARD_MS, java.util.concurrent.TimeUnit.MILLISECONDS));
                    awaitHostAction(game);
                    // The lost transport reports only after its timeout, well after the failure.
                    org.robolectric.shadows.ShadowLooper.idleMainLooper(9, java.util.concurrent.TimeUnit.SECONDS);
                    game.connectionTerminated(-1);
                    assertTrue(request + " ended in an error or a reconnect instead of closing", game.isFinishing());
                    assertNull(request, ReflectionHelpers.getField(game, "pendingRecovery"));
                    assertNull(request, ReflectionHelpers.getField(game, "streamMenu"));
                    assertEquals(0, RecoveryBridge.connectivityTests);
                    assertNull(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
                } finally {
                    cleanupRecovery(game);
                }
            }
        }
    }

    private static void awaitHostAction(Game game) throws InterruptedException {
        long deadline = System.nanoTime() + java.util.concurrent.TimeUnit.MILLISECONDS.toNanos(HANG_GUARD_MS);
        while (Boolean.TRUE.equals(ReflectionHelpers.getField(game, "hostActionInProgress"))
                && System.nanoTime() < deadline) {
            Thread.sleep(5);
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
        }
        assertFalse(ReflectionHelpers.getField(game, "hostActionInProgress"));
    }

    @Test public void aReasonlessDisconnectIsNotAttributedToThePc() {
        assertFalse(Game.terminationDescription(-1).contains("reported by the PC"));
        assertTrue(Game.terminationDescription(-1).startsWith("-1 ("));
        assertTrue(Game.terminationDescription(0x80030042).contains("reported by the PC"));
        assertTrue(Game.terminationDescription(MoonBridge.CLIENT_ERROR_VULKAN_RENDERER).contains("Vulkan"));
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aSessionThePcEndsAfterLeavingIsNeverResumed() throws Exception {
        Game game = establishedGame();
        try {
            game.onPause();
            game.onStop();
            assertNotNull(ReflectionHelpers.getField(game, "stoppedStreamResumeIntent"));
            // The termination callback was already on its way when the stream stopped.
            game.connectionTerminated(MoonBridge.ML_ERROR_GRACEFUL_TERMINATION);
            assertTrue(game.isFinishing());
            game.onResume();
            assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
            org.robolectric.shadows.ShadowLooper.idleMainLooper(15, java.util.concurrent.TimeUnit.SECONDS);
            assertNull(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void explicitHostFailuresNeverBecomeGracefulExitsEvenWhenTheHostIsIdle() {
        for (int error : new int[] {MoonBridge.ML_ERROR_PROTECTED_CONTENT, MoonBridge.ML_ERROR_FRAME_CONVERSION}) {
            Game game = establishedGame();
            ReflectionHelpers.setField(game, "conn", new MuteConnection() {
                @Override public boolean hasEndedSession() { throw new AssertionError("Explicit failure probed the host"); }
            });
            try {
                game.connectionTerminated(error);
                assertFalse(game.isFinishing());
                assertNotNull(ReflectionHelpers.getField(game, "streamMenu"));
                assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
            } finally {
                cleanupRecovery(game);
            }
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aReasonOnlyTheHostKnowsIsNotProbedAndStillReturnsToTheList() {
        Game game = establishedGame();
        ReflectionHelpers.setField(game, "conn", new MuteConnection() {
            @Override public boolean hasEndedSession() { throw new AssertionError("Explicit failure probed the host"); }
        });
        try {
            game.connectionTerminated(503);
            assertTrue(game.isFinishing());
            assertNull(ReflectionHelpers.getField(game, "streamMenu"));
            assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
            assertEquals("Connection Terminated\nError code: 503",
                    org.robolectric.shadows.ShadowToast.getTextOfLatestToast());
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aHostShutdownOrLostConnectionReturnsToTheListWithAMessageInsteadOfADialog() {
        // -1 is an ENet disconnect, 110 a connection timeout, 503 and 0x80030020 reasons from the PC.
        for (int error : new int[] {-1, 110, 10054, 503, 0x80030020}) {
            Game game = establishedGame();
            PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
            prefs.autoReconnect = false;
            ReflectionHelpers.setField(game, "conn", new MuteConnection() {
                @Override public boolean hasEndedSession() { return false; }
            });
            try {
                game.connectionTerminated(error);
                assertTrue("code " + error, game.isFinishing());
                assertNull(ReflectionHelpers.getField(game, "streamMenu"));
                assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
                // Android 12+ shows two lines of a text toast: the title and the code.
                String code = Math.abs(error) > 1000 ? Integer.toHexString(error) : Integer.toString(error);
                assertEquals("code " + error, "Connection Terminated\nError code: " + code,
                        org.robolectric.shadows.ShadowToast.getTextOfLatestToast());
            } finally {
                cleanupRecovery(game);
            }
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void usedUpAutomaticReconnectingKeepsTheDialogWithItsReconnectButton() {
        Game game = establishedGame();
        PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
        prefs.autoReconnect = true;
        ReflectionHelpers.setField(game, "conn", new MuteConnection() {
            @Override public boolean hasEndedSession() { return false; }
        });
        long now = SystemClock.elapsedRealtime();
        ReflectionHelpers.setField(game, "recoveryPolicy", new StreamRecoveryPolicy(StreamRecoveryPolicy.MAX_ATTEMPTS, now));
        ReflectionHelpers.setField(game, "connectedAt", now);
        try {
            game.connectionTerminated(-1);
            assertFalse(game.isFinishing());
            assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
            android.app.Dialog dialog = ReflectionHelpers.getField(game, "streamMenu");
            assertNotNull(dialog);
            assertTrue(dialog.isShowing());
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aGraceTimeThatMayHaveRunOutKeepsTheDialogForItsAdvice() {
        Game game = establishedGame();
        PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
        prefs.autoReconnect = false;
        prefs.networkGraceSeconds = 30;
        ReflectionHelpers.setField(game, "conn", new MuteConnection() {
            @Override public boolean hasEndedSession() { return false; }
        });
        try {
            game.connectionTerminated(-1);
            assertFalse(game.isFinishing());
            assertNotNull(ReflectionHelpers.getField(game, "streamMenu"));
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void mediaFailuresKeepTheirRecoveryBehaviorWithoutBeingReclassifiedAsHostExits() throws Exception {
        for (int error : new int[] {MoonBridge.ML_ERROR_NO_VIDEO_TRAFFIC, MoonBridge.ML_ERROR_NO_VIDEO_FRAME,
                MoonBridge.ML_ERROR_UNEXPECTED_EARLY_TERMINATION}) {
            Game game = establishedGame();
            ReflectionHelpers.setField(game, "conn", new MuteConnection() {
                @Override public boolean hasEndedSession() { throw new AssertionError("Media failure probed the host"); }
            });
            try {
                game.connectionTerminated(error);
                assertFalse(game.isFinishing());
                assertNull(ReflectionHelpers.getField(game, "streamMenu"));
                assertNotNull(ReflectionHelpers.getField(game, "pendingRecovery"));
                assertTrue(awaitRecoveryLaunch(game, 1).getBooleanExtra("RecoveryMediaFailure", false));
            } finally {
                cleanupRecovery(game);
            }
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void mediaFailureProvenanceSurvivesStartupRetriesUntilAManualReconnect() throws Exception {
        Game game = recoveryGame();
        game.setIntent(new Intent().putExtra("RecoveryAttempts", 1).putExtra("RecoveryMediaFailure", true));
        ReflectionHelpers.setField(game, "connected", false);
        ReflectionHelpers.setField(game, "connecting", true);
        try {
            game.stageFailed("Game", MoonBridge.ML_PORT_FLAG_TCP_47984 | MoonBridge.ML_PORT_FLAG_TCP_47989, 0);
            assertTrue(awaitRecoveryLaunch(game, 1).getBooleanExtra("RecoveryMediaFailure", false));
            Intent manual = ReflectionHelpers.callInstanceMethod(game, "createReconnectIntent");
            assertFalse(manual.hasExtra("RecoveryMediaFailure"));
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void anEndedSessionDuringRecoveryStartupFinishesWithoutAnotherAttempt() {
        Game game = recoveryGame();
        game.setIntent(new Intent().putExtra("RecoveryAttempts", 1));
        ReflectionHelpers.setField(game, "connected", false);
        ReflectionHelpers.setField(game, "connecting", true);
        try {
            game.connectionTerminated(MoonBridge.ML_ERROR_GRACEFUL_TERMINATION);
            assertTrue(game.isFinishing());
            assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
            assertNull(ReflectionHelpers.getField(game, "streamMenu"));
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test public void lockResumePreservesCapturedIdentityOutputAndExistingRetryBudget() throws Exception {
        org.robolectric.shadows.ShadowSystemClock.advanceBy(java.time.Duration.ofSeconds(10));
        Game game = establishedGame();
        long started = SystemClock.elapsedRealtime() - 5000;
        game.setIntent(new Intent().putExtra(Game.EXTRA_APP_ID, 2147483501)
                .putExtra("RecoveryAttempts", 2).putExtra("RecoveryStarted", started)
                .putExtra("rubylight.outputDisplay", 42).putExtra("rubylight.outputResumeOnly", true));
        ReflectionHelpers.setField(game, "recoveryPolicy", new StreamRecoveryPolicy(2, started));
        try {
            game.onPause();
            long leftAt = SystemClock.elapsedRealtime();
            game.onStop();
            assertFalse(game.isFinishing());
            assertNotNull(ReflectionHelpers.getField(game, "stoppedStreamResumeIntent"));
            org.robolectric.shadows.ShadowLooper.idleMainLooper(10, java.util.concurrent.TimeUnit.SECONDS);
            assertNull(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
            // A shortcut or tile must not replace the identity captured when the session began.
            game.getIntent().putExtra(Game.EXTRA_APP_ID, 99);
            long returnedAt = SystemClock.elapsedRealtime();
            game.onResume();
            assertNull(ReflectionHelpers.getField(game, "stoppedStreamResumeIntent"));
            assertFalse(ReflectionHelpers.callInstanceMethod(game, "resumeStoppedStreamIfNeeded"));
            Intent retry = awaitRecoveryLaunch(game, 1);
            assertEquals(73, retry.getIntExtra(Game.EXTRA_APP_ID, 0));
            assertEquals("captured-game", retry.getStringExtra(Game.EXTRA_APP_UUID));
            assertEquals(42, retry.getIntExtra("rubylight.outputDisplay", 0));
            assertTrue(retry.getBooleanExtra("rubylight.outputResumeOnly", false));
            // The resume is not an attempt: it carries the budget as the trip began and its own deadline.
            assertArrayEquals(new long[] {2, started, leftAt, 0}, retry.getLongArrayExtra("rubylight.tripBudget"));
            assertEquals(returnedAt, retry.getLongExtra("rubylight.tripResumeStarted", 0));
            StreamRecoveryPolicy old = ReflectionHelpers.getField(game, "recoveryPolicy");
            assertEquals(2, old.attempts());
            assertEquals(started, old.startedAt());
            Game replacement = Robolectric.buildActivity(Game.class).get();
            assertNull(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
            // The failure being recovered keeps its two attempts; its window starts later by the time away.
            long createdAt = SystemClock.elapsedRealtime();
            StreamRecoveryPolicy restored = startStream(replacement, retry);
            assertTrue(ReflectionHelpers.callInstanceMethod(replacement, "isResumeOnlyStream"));
            assertEquals(2, restored.attempts());
            assertEquals(started + (createdAt - leftAt), restored.startedAt());
            assertEquals(2, replacement.getIntent().getIntExtra("RecoveryAttempts", 0));
            assertFalse(replacement.getIntent().hasExtra("rubylight.tripBudget"));
        } finally {
            cleanupRecovery(game);
        }
    }

    /** The replacement activity starts with {@code intent}: the budget onCreate reads. */
    private static StreamRecoveryPolicy startStream(Game stream, Intent intent) {
        stream.setIntent(intent);
        stream.readRecoveryBudget(SystemClock.elapsedRealtime());
        return ReflectionHelpers.getField(stream, "recoveryPolicy");
    }

    /**
     * Leaves for {@code awayMs} (Files, Home) and returns: the resume runs at once and, in the
     * replacement activity, connects with the budget the stream had when it was left.
     */
    private Game leaveAndReturn(Game stream, long awayMs) throws InterruptedException {
        long leftAt = SystemClock.elapsedRealtime();
        Intent retry = leaveAndComeBack(stream, awayMs);
        StreamRecoveryPolicy before = ReflectionHelpers.getField(stream, "recoveryPolicy");
        Game resumed = establishedGame();
        long createdAt = SystemClock.elapsedRealtime();
        StreamRecoveryPolicy budget = startStream(resumed, retry);
        assertTrue(ReflectionHelpers.callInstanceMethod(resumed, "isResumeOnlyStream"));
        assertEquals("the trip spent none of the failure budget", before.attempts(), budget.attempts());
        assertEquals(before.attempts() == 0 ? 0 : before.startedAt() + (createdAt - leftAt), budget.startedAt());
        resumed.onStreamConnected(SystemClock.elapsedRealtime());
        return resumed;
    }

    /** Leaves {@code stream} for {@code awayMs} and returns; the resume's intent, which spent nothing. */
    private static Intent leaveAndComeBack(Game stream, long awayMs) throws InterruptedException {
        stream.onPause();
        stream.onStop();
        StreamRecoveryPolicy before = ReflectionHelpers.getField(stream, "recoveryPolicy");
        org.robolectric.shadows.ShadowSystemClock.advanceBy(java.time.Duration.ofMillis(awayMs));
        long returnedAt = SystemClock.elapsedRealtime();
        stream.onResume();
        assertNotNull("the return was refused", ReflectionHelpers.getField(stream, "pendingRecovery"));
        // Its fixed one-second delay, whatever the failure budget holds.
        Intent retry = awaitRecoveryLaunch(stream, 1);
        StreamRecoveryPolicy after = ReflectionHelpers.getField(stream, "recoveryPolicy");
        assertEquals("the resume took an attempt", before.attempts(), after.attempts());
        assertEquals(before.startedAt(), after.startedAt());
        cleanupRecovery(stream);
        assertEquals(returnedAt, retry.getLongExtra("rubylight.tripResumeStarted", 0));
        return retry;
    }

    @Test public void userTripsWithinAMinuteEachResumeWithoutSpendingTheFailureBudget() throws Exception {
        // R5 on the Retroid: Files, Back, under 30 s of stream, Files again; two and three trips.
        for (long[] trips : new long[][] {{15000, 45000}, {15000, 15000, 15000}}) {
            Game stream = establishedGame();
            try {
                for (long away : trips) {
                    stream = leaveAndReturn(stream, away);
                    org.robolectric.shadows.ShadowSystemClock.advanceBy(java.time.Duration.ofSeconds(20));
                }
            } finally {
                cleanupRecovery(stream);
            }
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void tripsAfterARecoveredBlipEachResumeAndCountOnlyConnectedTime() throws Exception {
        org.robolectric.shadows.ShadowSystemClock.advanceBy(java.time.Duration.ofSeconds(100));
        // A Wi-Fi blip at t=0 recovered by t=3 (attempt 1), then Files every 25 s.
        long blip = SystemClock.elapsedRealtime();
        Game stream = establishedGame();
        stream.setIntent(new Intent().putExtra("RecoveryAttempts", 1).putExtra("RecoveryStarted", blip));
        ReflectionHelpers.setField(stream, "recoveryPolicy", new StreamRecoveryPolicy(1, blip));
        org.robolectric.shadows.ShadowSystemClock.advanceBy(java.time.Duration.ofSeconds(3));
        stream.onStreamConnected(SystemClock.elapsedRealtime());
        try {
            org.robolectric.shadows.ShadowSystemClock.advanceBy(java.time.Duration.ofSeconds(20));
            stream = leaveAndReturn(stream, 5000);
            // The blip's budget is back, its window run only by the 3 s startup and the 20 s connected.
            StreamRecoveryPolicy budget = ReflectionHelpers.getField(stream, "recoveryPolicy");
            assertEquals(1, budget.attempts());
            assertEquals(23000, SystemClock.elapsedRealtime() - budget.startedAt());
            for (int trip = 0; trip < 3; trip++) {
                org.robolectric.shadows.ShadowSystemClock.advanceBy(java.time.Duration.ofSeconds(20));
                stream = leaveAndReturn(stream, 5000);
            }
            // 40 s connected in all by the second trip: as without the trips, the blip's budget is
            // done and a failure now has a full one.
            assertTrue(SystemClock.elapsedRealtime() - blip > StreamRecoveryPolicy.WINDOW_MS);
            assertEquals(0, ((StreamRecoveryPolicy) ReflectionHelpers.getField(stream, "recoveryPolicy")).attempts());
            org.robolectric.shadows.ShadowSystemClock.advanceBy(java.time.Duration.ofSeconds(5));
            long failedAt = SystemClock.elapsedRealtime();
            ReflectionHelpers.setField(stream, "conn", new MuteConnection() {
                @Override public boolean hasEndedSession() { return false; }
            });
            stream.connectionTerminated(-1);
            Intent retry = awaitRecoveryLaunch(stream, 1);
            assertEquals(1, retry.getIntExtra("RecoveryAttempts", 0));
            assertEquals(failedAt, retry.getLongExtra("RecoveryStarted", 0));
        } finally {
            cleanupRecovery(stream);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void neitherTimeAwayNorTheResumeCountsAsConnectedTimeAndTheCarryCountsOnce() throws Exception {
        org.robolectric.shadows.ShadowSystemClock.advanceBy(java.time.Duration.ofSeconds(100));
        // A blip at t=0 recovered by t=3 (attempt 1); 10 s connected, a minute away, 10 s connected.
        long blip = SystemClock.elapsedRealtime();
        Game stream = establishedGame();
        stream.setIntent(new Intent().putExtra("RecoveryAttempts", 1).putExtra("RecoveryStarted", blip));
        ReflectionHelpers.setField(stream, "recoveryPolicy", new StreamRecoveryPolicy(1, blip));
        org.robolectric.shadows.ShadowSystemClock.advanceBy(java.time.Duration.ofSeconds(3));
        stream.onStreamConnected(SystemClock.elapsedRealtime());
        try {
            org.robolectric.shadows.ShadowSystemClock.advanceBy(java.time.Duration.ofSeconds(10));
            long leftAt = SystemClock.elapsedRealtime();
            stream = leaveAndReturn(stream, 60000);
            long resumedAt = SystemClock.elapsedRealtime();
            org.robolectric.shadows.ShadowSystemClock.advanceBy(java.time.Duration.ofSeconds(10));
            ReflectionHelpers.setField(stream, "conn", new MuteConnection() {
                @Override public boolean hasEndedSession() { return false; }
            });
            stream.connectionTerminated(-1);
            // 20 s connected in all, short of the 30 s that would earn a fresh budget: had the
            // minute away, the resume's startup or the carried 10 s twice counted, it would be one.
            Intent retry = awaitRecoveryLaunch(stream, 2);
            assertEquals(2, retry.getIntExtra("RecoveryAttempts", 0));
            // The blip's window, its start later by the time from leaving until the resume started.
            assertEquals(blip + (resumedAt - leftAt), retry.getLongExtra("RecoveryStarted", 0));
        } finally {
            cleanupRecovery(stream);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aStartupFailureOfAResumeAfterATripWithNoFailureRetriesOnAFreshBudget() throws Exception {
        Intent resume = leaveAndComeBack(establishedGame(), 10000);
        // The replacement activity before connectionStarted, its budget read as onCreate does.
        Game resumed = recoveryGame();
        StreamRecoveryPolicy budget = startStream(resumed, resume);
        assertEquals(0, budget.attempts());
        ReflectionHelpers.setField(resumed, "connected", false);
        ReflectionHelpers.setField(resumed, "connecting", true);
        RecoveryBridge.connectivityTests = 0;
        try {
            long failedAt = SystemClock.elapsedRealtime();
            resumed.stageFailed("Control stream establishment", MoonBridge.ML_PORT_FLAG_UDP_47999, 110);
            // Still a recovery attempt with no attempt spent: it retries instead of an error.
            assertNotNull(ReflectionHelpers.getField(resumed, "pendingRecovery"));
            assertNull(ReflectionHelpers.getField(resumed, "streamMenu"));
            assertEquals(0, RecoveryBridge.connectivityTests);
            Intent retry = awaitRecoveryLaunch(resumed, 1);
            assertEquals(1, retry.getIntExtra("RecoveryAttempts", 0));
            assertEquals(failedAt, retry.getLongExtra("RecoveryStarted", 0));
            assertFalse(retry.hasExtra("rubylight.tripBudget"));
            assertFalse(retry.hasExtra("rubylight.tripResumeStarted"));
        } finally {
            cleanupRecovery(resumed);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aRealFailureAfterUserTripsHasItsFullBudgetAndIsStillRefusedAfterItsMinute() throws Exception {
        Game stream = leaveAndReturn(leaveAndReturn(establishedGame(), 15000), 15000);
        try {
            long firstFailure = 0;
            int recoveries = 0;
            long[] delays = {1, 2, 4, 8, 12};
            while (true) {
                // A flapping connection: up 15 s at a time, never the 30 s that earns a new budget.
                org.robolectric.shadows.ShadowSystemClock.advanceBy(java.time.Duration.ofSeconds(15));
                if (firstFailure == 0) firstFailure = SystemClock.elapsedRealtime();
                ReflectionHelpers.setField(stream, "conn", new MuteConnection() {
                    @Override public boolean hasEndedSession() { return false; }
                });
                stream.connectionTerminated(-1);
                if (ReflectionHelpers.getField(stream, "pendingRecovery") == null) break;
                Intent retry = awaitRecoveryLaunch(stream, (int) delays[recoveries]);
                cleanupRecovery(stream);
                recoveries++;
                assertEquals(recoveries, retry.getIntExtra("RecoveryAttempts", 0));
                // The budget starts at the first failure, not at either trip.
                assertEquals(firstFailure, retry.getLongExtra("RecoveryStarted", 0));
                assertFalse(retry.hasExtra("rubylight.tripBudget"));
                assertFalse(retry.hasExtra("rubylight.tripResumeStarted"));
                stream = establishedGame();
                startStream(stream, retry);
                stream.onStreamConnected(SystemClock.elapsedRealtime());
            }
            // Failures at 0, 16, 33 and 52 s: the fourth attempt would end past the minute.
            assertEquals(3, recoveries);
            assertTrue(SystemClock.elapsedRealtime() - firstFailure < StreamRecoveryPolicy.WINDOW_MS);
            assertFalse(stream.isFinishing());
            android.app.Dialog dialog = ReflectionHelpers.getField(stream, "streamMenu");
            assertNotNull(dialog);
            assertTrue(dialog.isShowing());
        } finally {
            cleanupRecovery(stream);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aFailureWhileTheResumeStartsContinuesTheBudgetFromBeforeTheTrip() throws Exception {
        org.robolectric.shadows.ShadowSystemClock.advanceBy(java.time.Duration.ofSeconds(100));
        long started = SystemClock.elapsedRealtime() - 5000;
        Game stream = establishedGame();
        ReflectionHelpers.setField(stream, "recoveryPolicy", new StreamRecoveryPolicy(2, started));
        Game resumed = leaveAndReturn(stream, 10000);
        StreamRecoveryPolicy budget = ReflectionHelpers.getField(resumed, "recoveryPolicy");
        long budgetStart = budget.startedAt();
        // connectionStarted has not come yet.
        ReflectionHelpers.setField(resumed, "connected", false);
        ReflectionHelpers.setField(resumed, "connecting", true);
        ReflectionHelpers.setField(resumed, "connectedAt", -1L);
        ReflectionHelpers.setField(resumed, "conn", new MuteConnection() {
            @Override public boolean hasEndedSession() { return false; }
        });
        try {
            resumed.connectionTerminated(-1);
            assertNotNull(ReflectionHelpers.getField(resumed, "pendingRecovery"));
            Intent retry = awaitRecoveryLaunch(resumed, 4);
            assertEquals(3, retry.getIntExtra("RecoveryAttempts", 0));
            assertEquals(budgetStart, retry.getLongExtra("RecoveryStarted", 0));
            assertFalse(retry.hasExtra("rubylight.tripBudget"));
            assertFalse(retry.hasExtra("rubylight.tripResumeStarted"));
        } finally {
            cleanupRecovery(resumed);
        }
    }

    @Test public void unpluggingTheDisplayWhileAwayIsAUserTrip() {
        Game game = establishedGame();
        try {
            game.onPause();
            ReflectionHelpers.callInstanceMethod(game, "returnVideoToDevice");
            Intent resume = ReflectionHelpers.getField(game, "stoppedStreamResumeIntent");
            assertNotNull(resume);
            assertTrue(resume.hasExtra("rubylight.tripBudget"));
            assertFalse(resume.hasExtra("rubylight.outputDisplay"));
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test public void aFailureReportedAsTheUserLeavesIsNoUserTrip() {
        Game game = establishedGame();
        try {
            game.onPause();
            // connectionTerminated marked the stream on its own thread; its UI half has not run yet.
            ReflectionHelpers.setField(game, "streamTerminated", true);
            game.onStop();
            Intent resume = ReflectionHelpers.getField(game, "stoppedStreamResumeIntent");
            assertNotNull(resume);
            assertFalse(resume.hasExtra("rubylight.tripBudget"));
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aConnectionLostWhileAwaySpendsTheFailureBudget() {
        org.robolectric.shadows.ShadowSystemClock.advanceBy(java.time.Duration.ofSeconds(100));
        Game game = establishedGame();
        // Paused for the 10 s away it would have room for one more attempt; counted, it has none.
        long started = SystemClock.elapsedRealtime() - 40000;
        ReflectionHelpers.setField(game, "recoveryPolicy", new StreamRecoveryPolicy(4, started));
        ReflectionHelpers.setField(game, "conn", new MuteConnection() {
            @Override public boolean hasEndedSession() { return false; }
        });
        try {
            game.onPause();
            game.connectionTerminated(-1);
            game.onStop();
            Intent resume = ReflectionHelpers.getField(game, "stoppedStreamResumeIntent");
            assertNotNull(resume);
            assertFalse("a lost connection is no user trip", resume.hasExtra("rubylight.tripBudget"));
            org.robolectric.shadows.ShadowSystemClock.advanceBy(java.time.Duration.ofSeconds(10));
            game.onResume();
            assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
            assertNull(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void timeLockedResumesButCannotResetAnExhaustedUnstableRecoveryWindow() throws Exception {
        org.robolectric.shadows.ShadowSystemClock.advanceBy(java.time.Duration.ofSeconds(100));
        Game game = establishedGame();
        long started = SystemClock.elapsedRealtime() - 58000;
        ReflectionHelpers.setField(game, "recoveryPolicy", new StreamRecoveryPolicy(4, started));
        // Two minutes locked: the return still resumes, and the nearly spent window stays nearly spent.
        Game resumed = leaveAndReturn(game, 120000);
        StreamRecoveryPolicy budget = ReflectionHelpers.getField(resumed, "recoveryPolicy");
        assertEquals(4, budget.attempts());
        assertEquals(58000, SystemClock.elapsedRealtime() - budget.startedAt());
        ReflectionHelpers.setField(resumed, "conn", new MuteConnection() {
            @Override public boolean hasEndedSession() { return false; }
        });
        try {
            resumed.connectionTerminated(-1);
            assertNull(ReflectionHelpers.getField(resumed, "pendingRecovery"));
            android.app.Dialog dialog = ReflectionHelpers.getField(resumed, "streamMenu");
            assertNotNull(dialog);
            assertTrue(dialog.isShowing());
            assertNull(org.robolectric.Shadows.shadowOf(resumed).getNextStartedActivity());
        } finally {
            cleanupRecovery(resumed);
        }
    }

    @Test public void leavingADelayedLockResumeCancelsItBeforeAnotherActivityStarts() throws Exception {
        Game game = establishedGame();
        try {
            game.onPause();
            game.onStop();
            game.onResume();
            com.limelight.utils.SpinnerDialog spinner = ReflectionHelpers.getField(game, "spinner");
            assertNotNull(spinner);
            spinner.onCancel(null);
            assertTrue(game.isFinishing());
            org.robolectric.shadows.ShadowLooper.idleMainLooper(15, java.util.concurrent.TimeUnit.SECONDS);
            assertNull(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test public void lifecycleResumeRequiresAnEstablishedEligibleSessionAndEnabledRecovery() {
        for (String condition : new String[] {"disabled", "quit", "failure", "uncaptured", "connecting"}) {
            Game game = establishedGame();
            if (condition.equals("disabled")) {
                PreferenceConfiguration config = ReflectionHelpers.getField(game, "prefConfig");
                config.autoReconnect = false;
            } else if (condition.equals("quit")) game.finish();
            else if (condition.equals("failure")) ReflectionHelpers.setField(game, "displayedFailureDialog", true);
            else if (condition.equals("uncaptured")) ReflectionHelpers.setField(game, "recoveryAppId", 0);
            else {
                ReflectionHelpers.setField(game, "connected", false);
                ReflectionHelpers.setField(game, "connecting", true);
            }
            ReflectionHelpers.callInstanceMethod(game, "rememberStreamForResume",
                    ReflectionHelpers.ClassParameter.from(boolean.class, true));
            assertNull(condition, ReflectionHelpers.getField(game, "stoppedStreamResumeIntent"));
        }
    }

    @Test public void stoppingDisabledOrIneligibleSessionsStillFinishesTheirActivity() {
        for (String condition : new String[] {"disabled", "monitor", "input", "control", "failure", "uncaptured", "connecting"}) {
            Game game = establishedGame();
            if (condition.equals("disabled")) {
                PreferenceConfiguration config = ReflectionHelpers.getField(game, "prefConfig");
                config.autoReconnect = false;
            } else if (condition.equals("monitor")) {
                ReflectionHelpers.setField(game, "app", new NvApp("Monitor", 2147483505, false));
            } else if (condition.equals("input")) {
                ReflectionHelpers.setField(game, "app", new NvApp("Input", 2147483506, false));
            } else if (condition.equals("control")) {
                ReflectionHelpers.setField(game, "app", new NvApp("Terminate", 2147483504, false));
            } else if (condition.equals("failure")) {
                ReflectionHelpers.setField(game, "displayedFailureDialog", true);
            } else if (condition.equals("uncaptured")) {
                ReflectionHelpers.setField(game, "recoveryAppId", 0);
            } else {
                ReflectionHelpers.setField(game, "connected", false);
                ReflectionHelpers.setField(game, "connecting", true);
            }
            try {
                game.onPause();
                game.onStop();
                assertTrue(condition, game.isFinishing());
                assertNull(condition, ReflectionHelpers.getField(game, "stoppedStreamResumeIntent"));
                assertNull(condition, org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
            } finally {
                cleanupRecovery(game);
            }
        }
    }

    @Test public void withRecoveryOffLeavingDuringAReconnectEndsTheStream() throws Exception {
        Game game = establishedGame();
        PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
        prefs.autoReconnect = false;
        java.util.concurrent.CountDownLatch stopEntered = new java.util.concurrent.CountDownLatch(1);
        java.util.concurrent.CountDownLatch stopReleased = new java.util.concurrent.CountDownLatch(1);
        ReflectionHelpers.setField(game, "conn", new MuteConnection() {
            @Override public void stop() {
                stopEntered.countDown();
                try { stopReleased.await(HANG_GUARD_MS, java.util.concurrent.TimeUnit.MILLISECONDS); }
                catch (InterruptedException interrupted) { Thread.currentThread().interrupt(); }
            }
        });
        try {
            ReflectionHelpers.callInstanceMethod(game, "reconnectStream");
            assertTrue(stopEntered.await(HANG_GUARD_MS, java.util.concurrent.TimeUnit.MILLISECONDS));
            // Home while the old connection is still closing: the stream ends, as before recovery.
            game.onPause();
            game.onStop();
            assertTrue(game.isFinishing());
            stopReleased.countDown();
            settleHandoff(game);
            assertNull(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
        } finally {
            stopReleased.countDown();
            cleanupRecovery(game);
        }
    }

    @Test public void destroyingAnEstablishedSurfaceWaitsForForegroundBeforeResuming() {
        Game game = establishedGame();
        ReflectionHelpers.setField(game, "surfaceCreated", true);
        game.onPause();
        try {
            ReflectionHelpers.callInstanceMethod(game, "destroyVideoSurface");
            assertFalse(game.isFinishing());
            assertNotNull(ReflectionHelpers.getField(game, "stoppedStreamResumeIntent"));
            assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
            game.onResume();
            assertNotNull(ReflectionHelpers.getField(game, "pendingRecovery"));
        } finally {
            cleanupRecovery(game);
        }
    }

    // The sweep's Wi-Fi blip: the stream's network is lost (the overlay says so at once), then a
    // network returns that the stream does not roam onto. The recovery starts with no delay instead
    // of after the connection's timeout, once the PC answers, and resumes the captured game.
    @Test public void aNetworkBackWithoutRoamingReconnectsAtOnce() throws Exception {
        org.robolectric.shadows.ShadowSystemClock.advanceBy(java.time.Duration.ofSeconds(10));
        Game game = establishedGame();
        android.widget.TextView overlay = new android.widget.TextView(game);
        ReflectionHelpers.setField(game, "notificationOverlayView", overlay);
        try (java.net.ServerSocket pc = new java.net.ServerSocket(0, 4, java.net.InetAddress.getLoopbackAddress())) {
            game.setIntent(new Intent().putExtra(Game.EXTRA_HOST, "127.0.0.1").putExtra(Game.EXTRA_PORT, pc.getLocalPort()));
            game.streamNetworkLost();
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertEquals(game.getString(R.string.stream_network_lost), overlay.getText().toString());
            assertEquals(android.view.View.VISIBLE, overlay.getVisibility());
            // A poor-connection update meanwhile does not replace the notice.
            game.connectionStatusUpdate(MoonBridge.CONN_STATUS_OKAY);
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertEquals(android.view.View.VISIBLE, overlay.getVisibility());

            game.streamNetworkReturned(false);
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertEquals(android.view.View.GONE, overlay.getVisibility());
            // No time passes: the attempt does not wait for a backoff delay.
            Intent retry = awaitStartedActivity(game);
            assertEquals(73, retry.getIntExtra(Game.EXTRA_APP_ID, 0));
            assertEquals("captured-game", retry.getStringExtra(Game.EXTRA_APP_UUID));
            assertEquals(1, retry.getIntExtra("RecoveryAttempts", 0));
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test public void aNetworkBackThatTheStreamRoamsOntoKeepsTheStream() throws Exception {
        Game game = establishedGame();
        android.widget.TextView overlay = new android.widget.TextView(game);
        ReflectionHelpers.setField(game, "notificationOverlayView", overlay);
        try {
            game.streamNetworkLost();
            game.streamNetworkReturned(true);
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertEquals(android.view.View.GONE, overlay.getVisibility());
            assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
            assertFalse(ReflectionHelpers.<Boolean>getField(game, "displayedFailureDialog"));
            // With a network drop tolerance set, the tolerance decides: no reconnect either.
            PreferenceConfiguration config = ReflectionHelpers.getField(game, "prefConfig");
            config.networkGraceSeconds = 20;
            game.streamNetworkLost();
            game.streamNetworkReturned(false);
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
        } finally {
            cleanupRecovery(game);
        }
    }

    private static Intent awaitRecoveryLaunch(Game game, int delaySeconds) throws InterruptedException {
        join(ReflectionHelpers.getField(game, "connectionStopThread"));
        org.robolectric.shadows.ShadowLooper.idleMainLooper(delaySeconds, java.util.concurrent.TimeUnit.SECONDS);
        return awaitStartedActivity(game);
    }

    /** The activity the hand-off started, once its workers have posted it; only a hang fails. */
    private static Intent awaitStartedActivity(Game game) throws InterruptedException {
        long deadline = System.nanoTime() + java.util.concurrent.TimeUnit.MILLISECONDS.toNanos(HANG_GUARD_MS);
        while (true) {
            settleHandoff(game);
            Intent retry = org.robolectric.Shadows.shadowOf(game).getNextStartedActivity();
            if (retry != null) return retry;
            if (System.nanoTime() > deadline) fail("Recovery did not create the replacement activity");
            Thread.sleep(5);
        }
    }

    /**
     * Joins the stream's stop and the hand-off workers that wait for it, so whatever they post to
     * the UI thread is queued, then runs it: a late continuation shows here, not after the test.
     */
    private static void settleHandoff(Game game) throws InterruptedException {
        join(ReflectionHelpers.getField(game, "connectionStopThread"));
        for (Thread thread : Thread.getAllStackTraces().keySet()) {
            if (HANDOFF_WORKERS.contains(thread.getName())) join(thread);
        }
        org.robolectric.shadows.ShadowLooper.idleMainLooper();
    }

    private static void join(Thread thread) throws InterruptedException {
        if (thread == null) return;
        thread.join(HANG_GUARD_MS);
        assertFalse(thread.getName() + " did not finish", thread.isAlive());
    }

    private static void cleanupRecovery(Game game) {
        ReflectionHelpers.callInstanceMethod(game, "cancelAutomaticRecovery");
        android.app.Dialog dialog = ReflectionHelpers.getField(game, "streamMenu");
        if (dialog != null) dialog.dismiss();
        com.limelight.utils.SpinnerDialog.closeDialogs(game);
    }

    @Test public void packedChromaFailureReconnectsOnceWithoutPackedEvenWithAutoReconnectOff() throws Exception {
        Game game = recoveryGame();
        PreferenceConfiguration config = ReflectionHelpers.getField(game, "prefConfig");
        config.autoReconnect = false;
        config.experimentalPacked444 = true;
        try {
            assertTrue(ReflectionHelpers.callInstanceMethod(game, "reconnectWithoutPackedChroma",
                    ReflectionHelpers.ClassParameter.from(String.class, "the GPU reconstruction could not keep up")));
            Intent retry = awaitRecoveryLaunch(game, 1);
            assertTrue(retry.getBooleanExtra("PackedChromaOff", false));
            assertEquals(1, retry.getIntExtra("RecoveryAttempts", 0));
            assertEquals("the setting itself is untouched", true, config.experimentalPacked444);
            // The replacement session turns packed off for itself only.
            assertFalse(com.limelight.binding.video.PackedFallback.effective(config.experimentalPacked444, retry.getBooleanExtra("PackedChromaOff", false)));
            // A session that already runs without packed chroma does not degrade again.
            Game replacement = recoveryGame();
            replacement.setIntent(retry);
            assertFalse(ReflectionHelpers.callInstanceMethod(replacement, "reconnectWithoutPackedChroma",
                    ReflectionHelpers.ClassParameter.from(String.class, "again")));
            assertNull(ReflectionHelpers.getField(replacement, "pendingRecovery"));
        } finally {
            cleanupRecovery(game);
        }
    }

    private static final class TerminationProbe {
        final java.util.concurrent.CountDownLatch ended = new java.util.concurrent.CountDownLatch(1);
        volatile Thread endedOn;
        volatile int ends;
        final com.limelight.nvstream.NvConnectionListener listener =
                (com.limelight.nvstream.NvConnectionListener) java.lang.reflect.Proxy.newProxyInstance(
                        com.limelight.nvstream.NvConnectionListener.class.getClassLoader(),
                        new Class<?>[] {com.limelight.nvstream.NvConnectionListener.class}, (proxy, method, args) -> {
                            if (method.getName().equals("connectionTerminated")) {
                                endedOn = Thread.currentThread();
                                ends++;
                                ended.countDown();
                            }
                            return null;
                        });
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void packedChromaFailureThatCannotReconnectEndsTheStreamOffTheUiThread() throws Exception {
        Game game = recoveryGame();
        // This session already runs without packed chroma, so a failure has nothing left to turn off.
        game.setIntent(new Intent().putExtra("PackedChromaOff", true));
        TerminationProbe probe = new TerminationProbe();
        ReflectionHelpers.setStaticField(MoonBridge.class, "connectionListener", probe.listener);
        try {
            ReflectionHelpers.callInstanceMethod(game, "onPackedChromaFailure",
                    ReflectionHelpers.ClassParameter.from(String.class, "the GPU path failed"));
            assertTrue(probe.ended.await(HANG_GUARD_MS, java.util.concurrent.TimeUnit.MILLISECONDS));
            // connectionTerminated asks the PC and tests the network: blocking calls, never on the UI thread.
            assertNotSame(Looper.getMainLooper().getThread(), probe.endedOn);
            assertEquals(1, probe.ends);
        } finally {
            ReflectionHelpers.setStaticField(MoonBridge.class, "connectionListener", null);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void packedChromaFailureDuringARecoveryMarksTheReplacementInsteadOfEndingTheStream() throws Exception {
        Game game = recoveryGame();
        TerminationProbe probe = new TerminationProbe();
        ReflectionHelpers.setStaticField(MoonBridge.class, "connectionListener", probe.listener);
        try {
            assertTrue(ReflectionHelpers.callInstanceMethod(game, "tryAutomaticRecovery"));
            ReflectionHelpers.callInstanceMethod(game, "onPackedChromaFailure",
                    ReflectionHelpers.ClassParameter.from(String.class, "the GPU path failed"));
            assertFalse("no second reconnect and no dialog over the recovery", probe.ended.await(300, java.util.concurrent.TimeUnit.MILLISECONDS));
            Intent retry = awaitRecoveryLaunch(game, 1);
            assertTrue(retry.getBooleanExtra("PackedChromaOff", false));
            assertEquals(0, probe.ends);
        } finally {
            ReflectionHelpers.setStaticField(MoonBridge.class, "connectionListener", null);
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aManualReconnectAfterAPackedFailureDuringARecoveryStaysWithoutPacked() {
        Game game = recoveryGame();
        ReflectionHelpers.setStaticField(MoonBridge.class, "connectionListener", new TerminationProbe().listener);
        try {
            assertTrue(ReflectionHelpers.callInstanceMethod(game, "tryAutomaticRecovery"));
            ReflectionHelpers.callInstanceMethod(game, "onPackedChromaFailure",
                    ReflectionHelpers.ClassParameter.from(String.class, "the GPU path failed"));
            // The recovery goes stale (cancelled, timed out) and the user taps Reconnect.
            ReflectionHelpers.callInstanceMethod(game, "cancelAutomaticRecovery");
            Intent reconnect = ReflectionHelpers.callInstanceMethod(game, "createReconnectIntent");
            assertTrue(reconnect.getBooleanExtra("PackedChromaOff", false));
            Intent resume = ReflectionHelpers.callInstanceMethod(game, "createStreamHandoffIntent",
                    ReflectionHelpers.ClassParameter.from(String.class, com.limelight.utils.LaunchSource.RETURN));
            assertTrue(resume.getBooleanExtra("PackedChromaOff", false));
        } finally {
            ReflectionHelpers.setStaticField(MoonBridge.class, "connectionListener", null);
            cleanupRecovery(game);
        }
    }

    @Test
    public void aStreamStartedAgainDoesNotNameTheGameItsFirstStartReplaced() {
        Game game = recoveryGame();
        try {
            game.setIntent(new Intent().putExtra(Game.EXTRA_REPLACED_APP_NAME, "Balatro"));
            // By the reconnect another device's game may be running: its confirmation must not name Balatro.
            Intent reconnect = ReflectionHelpers.callInstanceMethod(game, "createReconnectIntent");
            assertFalse(reconnect.hasExtra(Game.EXTRA_REPLACED_APP_NAME));
            Intent resume = ReflectionHelpers.callInstanceMethod(game, "createStreamHandoffIntent",
                    ReflectionHelpers.ClassParameter.from(String.class, com.limelight.utils.LaunchSource.RETURN));
            assertFalse(resume.hasExtra(Game.EXTRA_REPLACED_APP_NAME));
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void packedChromaFailureWhileTheStreamWaitsToResumeMarksTheResume() throws Exception {
        Game game = recoveryGame();
        ReflectionHelpers.setField(game, "foreground", false);
        Intent resume = new Intent();
        ReflectionHelpers.setField(game, "stoppedStreamResumeIntent", resume);
        TerminationProbe probe = new TerminationProbe();
        ReflectionHelpers.setStaticField(MoonBridge.class, "connectionListener", probe.listener);
        try {
            ReflectionHelpers.callInstanceMethod(game, "onPackedChromaFailure",
                    ReflectionHelpers.ClassParameter.from(String.class, "the GPU path failed"));
            assertTrue(resume.getBooleanExtra("PackedChromaOff", false));
            assertFalse(probe.ended.await(300, java.util.concurrent.TimeUnit.MILLISECONDS));
        } finally {
            ReflectionHelpers.setStaticField(MoonBridge.class, "connectionListener", null);
        }
    }

    @Test public void packedChromaFailureInTheBackgroundDoesNotReconnect() {
        Game game = recoveryGame();
        ReflectionHelpers.setField(game, "foreground", false);
        assertFalse(ReflectionHelpers.callInstanceMethod(game, "reconnectWithoutPackedChroma",
                ReflectionHelpers.ClassParameter.from(String.class, "gpu")));
        assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
    }

    // The RTSP handshake failed because full chroma cannot be streamed with this PC (code -106).
    private static void refuseFullChromaAtStart(Game game) {
        ReflectionHelpers.setField(game, "connected", false);
        ReflectionHelpers.setField(game, "connecting", true);
        game.stageFailed("RTSP handshake", MoonBridge.ML_PORT_FLAG_TCP_48010,
                MoonBridge.CLIENT_ERROR_PACKED_444_REFUSED);
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aFullChromaRefusalAtStartReconnectsOnceWithoutItInsteadOfAnError() throws Exception {
        // Also on a startup retry, where a transient-looking failure would otherwise retry as it was.
        for (int attempts : new int[] {0, 1}) {
            Game game = recoveryGame();
            long started = SystemClock.elapsedRealtime();
            game.setIntent(new Intent().putExtra("RecoveryAttempts", attempts).putExtra("RecoveryStarted", started));
            ReflectionHelpers.setField(game, "recoveryPolicy", new StreamRecoveryPolicy(attempts, started));
            PreferenceConfiguration config = ReflectionHelpers.getField(game, "prefConfig");
            config.autoReconnect = false;
            RecoveryBridge.connectivityTests = 0;
            try {
                refuseFullChromaAtStart(game);
                assertNull("no error dialog", ReflectionHelpers.getField(game, "streamMenu"));
                assertEquals("not a network failure", 0, RecoveryBridge.connectivityTests);
                // The usual backoff: 1 s, then 2 s.
                Intent retry = awaitRecoveryLaunch(game, attempts + 1);
                assertTrue(retry.getBooleanExtra("PackedChromaOff", false));
                assertEquals(attempts + 1, retry.getIntExtra("RecoveryAttempts", 0));
            } finally {
                cleanupRecovery(game);
            }
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aFullChromaRefusalOnAResumeOrRunningGameTileAlsoReconnectsWithoutIt() throws Exception {
        // Resume (2147483501) and Running Game (2147483507) tiles resolve to a game only once the
        // stream has started; a refusal comes before that.
        for (int tile : new int[] {2147483501, 2147483507}) {
            for (boolean running : new boolean[] {true, false}) {
                Game game = recoveryGame();
                NvApp app = new NvApp("Tile", tile, false);
                assertNotEquals(NvApp.Control.NONE, app.getControl());
                ReflectionHelpers.setField(game, "app", app);
                if (running) {
                    NvConnection connection = ReflectionHelpers.getField(game, "conn");
                    ComputerDetails host = new ComputerDetails();
                    host.runningGameId = 73;
                    host.runningGameUuid = "game-uuid";
                    ReflectionHelpers.setField(connection, "hostDetails", host);
                }
                game.setIntent(new Intent().putExtra(Game.EXTRA_APP_ID, tile));
                PreferenceConfiguration config = ReflectionHelpers.getField(game, "prefConfig");
                config.autoReconnect = false;
                try {
                    refuseFullChromaAtStart(game);
                    assertNull("no error dialog", ReflectionHelpers.getField(game, "streamMenu"));
                    Intent retry = awaitRecoveryLaunch(game, 1);
                    assertTrue(retry.getBooleanExtra("PackedChromaOff", false));
                    // The game the PC reported running, as a started stream resolves it; else the tile again.
                    assertEquals(running ? 73 : tile, retry.getIntExtra(Game.EXTRA_APP_ID, 0));
                    if (running) assertEquals("game-uuid", retry.getStringExtra(Game.EXTRA_APP_UUID));
                } finally {
                    cleanupRecovery(game);
                }
            }
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aSecondFullChromaRefusalShowsTheConnectionErrorWithoutAnotherRetry() {
        Game game = recoveryGame();
        // The session the first refusal started: already without full chroma.
        game.setIntent(new Intent().putExtra("PackedChromaOff", true).putExtra("RecoveryAttempts", 1));
        try {
            refuseFullChromaAtStart(game);
            assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
            assertTrue(ReflectionHelpers.getField(game, "displayedFailureDialog"));
            assertNotNull("the usual connection error", ReflectionHelpers.getField(game, "streamMenu"));
        } finally {
            cleanupRecovery(game);
        }
        assertTrue(Game.terminationDescription(MoonBridge.CLIENT_ERROR_PACKED_444_REFUSED).contains("full chroma"));
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
                try { stopReleased.await(HANG_GUARD_MS, java.util.concurrent.TimeUnit.MILLISECONDS); }
                catch (InterruptedException interrupted) { Thread.currentThread().interrupt(); }
            }
        });
        try {
            assertTrue(ReflectionHelpers.callInstanceMethod(game, "tryAutomaticRecovery"));
            assertTrue(stopEntered.await(HANG_GUARD_MS, java.util.concurrent.TimeUnit.MILLISECONDS));
            org.robolectric.Shadows.shadowOf(Looper.getMainLooper()).idleFor(java.time.Duration.ofSeconds(1));
            // The delayed retry is now waiting for teardown on a worker thread.
            assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
            ReflectionHelpers.callInstanceMethod(game, "stopConnection");
            stopReleased.countDown();
            // Any late posted continuation must observe the cancellation generation.
            settleHandoff(game);
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
            android.content.Intent retry = awaitStartedActivity(game);
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
                try { stopReleased.await(HANG_GUARD_MS, java.util.concurrent.TimeUnit.MILLISECONDS); }
                catch (InterruptedException interrupted) { Thread.currentThread().interrupt(); }
            }
        });
        try {
            if (automatic) assertTrue(ReflectionHelpers.callInstanceMethod(game, "tryAutomaticRecovery"));
            else ReflectionHelpers.callInstanceMethod(game, "reconnectStream");
            assertTrue(stopEntered.await(HANG_GUARD_MS, java.util.concurrent.TimeUnit.MILLISECONDS));
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
            settleHandoff(game);
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

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
            stopping.join(1000);
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
        stopping.join(1000);
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
                    try { answered.await(2, java.util.concurrent.TimeUnit.SECONDS); }
                    catch (InterruptedException e) { Thread.currentThread().interrupt(); }
                }
                // The game is still exiting, so the host does not look idle yet.
                @Override public boolean hasEndedSession() { return false; }
            });
            RecoveryBridge.connectivityTests = 0;
            try {
                ReflectionHelpers.callInstanceMethod(game, "quitStreamApp");
                assertTrue(quitting.await(1, java.util.concurrent.TimeUnit.SECONDS));
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
            assertTrue(refused.await(1, java.util.concurrent.TimeUnit.SECONDS));
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
                    assertTrue(failed.await(1, java.util.concurrent.TimeUnit.SECONDS));
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
        for (int i = 0; i < 100 && Boolean.TRUE.equals(ReflectionHelpers.getField(game, "hostActionInProgress")); i++) {
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
            game.onStop();
            assertFalse(game.isFinishing());
            assertNotNull(ReflectionHelpers.getField(game, "stoppedStreamResumeIntent"));
            org.robolectric.shadows.ShadowLooper.idleMainLooper(10, java.util.concurrent.TimeUnit.SECONDS);
            assertNull(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
            // A shortcut or tile must not replace the identity captured when the session began.
            game.getIntent().putExtra(Game.EXTRA_APP_ID, 99);
            game.onResume();
            assertNull(ReflectionHelpers.getField(game, "stoppedStreamResumeIntent"));
            assertFalse(ReflectionHelpers.callInstanceMethod(game, "resumeStoppedStreamIfNeeded"));
            Intent retry = awaitRecoveryLaunch(game, 4);
            assertEquals(73, retry.getIntExtra(Game.EXTRA_APP_ID, 0));
            assertEquals("captured-game", retry.getStringExtra(Game.EXTRA_APP_UUID));
            assertEquals(42, retry.getIntExtra("rubylight.outputDisplay", 0));
            assertTrue(retry.getBooleanExtra("rubylight.outputResumeOnly", false));
            assertEquals(3, retry.getIntExtra("RecoveryAttempts", 0));
            assertEquals(started, retry.getLongExtra("RecoveryStarted", 0));
            Game replacement = Robolectric.buildActivity(Game.class).get();
            replacement.setIntent(retry);
            assertTrue(ReflectionHelpers.callInstanceMethod(replacement, "isResumeOnlyStream"));
            assertNull(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test public void timeLockedCannotResetAnExhaustedUnstableRecoveryWindow() {
        org.robolectric.shadows.ShadowSystemClock.advanceBy(java.time.Duration.ofSeconds(100));
        Game game = establishedGame();
        long started = SystemClock.elapsedRealtime() - 58000;
        StreamRecoveryPolicy policy = new StreamRecoveryPolicy(4, started);
        ReflectionHelpers.setField(game, "recoveryPolicy", policy);
        try {
            game.onPause();
            game.onStop();
            org.robolectric.shadows.ShadowSystemClock.advanceBy(java.time.Duration.ofMinutes(2));
            game.onResume();
            assertEquals(4, policy.attempts());
            assertEquals(started, policy.startedAt());
            assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
            android.app.Dialog dialog = ReflectionHelpers.getField(game, "streamMenu");
            assertNotNull(dialog);
            assertTrue(dialog.isShowing());
            assertNull(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
        } finally {
            cleanupRecovery(game);
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
            ReflectionHelpers.callInstanceMethod(game, "rememberStreamForResume");
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
                try { stopReleased.await(2, java.util.concurrent.TimeUnit.SECONDS); }
                catch (InterruptedException interrupted) { Thread.currentThread().interrupt(); }
            }
        });
        try {
            ReflectionHelpers.callInstanceMethod(game, "reconnectStream");
            assertTrue(stopEntered.await(1, java.util.concurrent.TimeUnit.SECONDS));
            // Home while the old connection is still closing: the stream ends, as before recovery.
            game.onPause();
            game.onStop();
            assertTrue(game.isFinishing());
            stopReleased.countDown();
            Thread stopping = ReflectionHelpers.getField(game, "connectionStopThread");
            stopping.join(1000);
            for (int i = 0; i < 10; i++) {
                Thread.sleep(5);
                org.robolectric.Shadows.shadowOf(Looper.getMainLooper()).idle();
            }
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

    private static Intent awaitRecoveryLaunch(Game game, int delaySeconds) throws InterruptedException {
        Thread stopping = ReflectionHelpers.getField(game, "connectionStopThread");
        if (stopping != null) stopping.join(1000);
        org.robolectric.shadows.ShadowLooper.idleMainLooper(delaySeconds, java.util.concurrent.TimeUnit.SECONDS);
        for (int i = 0; i < 40; i++) {
            Thread.sleep(5);
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            Intent retry = org.robolectric.Shadows.shadowOf(game).getNextStartedActivity();
            if (retry != null) return retry;
        }
        fail("Recovery did not create the replacement activity");
        return null;
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
            assertTrue(probe.ended.await(5, java.util.concurrent.TimeUnit.SECONDS));
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
            Intent resume = ReflectionHelpers.callInstanceMethod(game, "createStreamHandoffIntent");
            assertTrue(resume.getBooleanExtra("PackedChromaOff", false));
        } finally {
            ReflectionHelpers.setStaticField(MoonBridge.class, "connectionListener", null);
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

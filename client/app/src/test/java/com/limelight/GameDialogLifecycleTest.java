package com.limelight;

import android.app.Application;
import android.app.Dialog;
import android.content.Intent;

import com.limelight.GameRecoveryTestSupport.RecoveryBridge;
import com.limelight.ui.NavigationDialogBuilder;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.util.ReflectionHelpers;

import static com.limelight.GameRecoveryTestSupport.*;
import static org.junit.Assert.*;

/**
 * Regression tests for stream dialog lifecycle (T8-11, T8-12):
 * <ul>
 *   <li>Terminal post-stream dialogs (Reconnect, Moved) survive {@code onPause()}/{@code onResume()}
 *       while non-terminal stream dialogs still dismiss on {@code onPause()}.</li>
 *   <li>{@link GameMenuActions#showStreamDialog} dismisses any previously open dialog and keeps
 *       {@code suppressPipRefCount} balanced.</li>
 * </ul>
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class GameDialogLifecycleTest {
    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void onPausePreservesTerminalReconnectAndMovedDialogsAcrossResume() throws Exception {
        Game game = establishedGame();
        try {
            game.connected = false;
            game.streamRecovery.showReconnectDialog(R.string.conn_terminated_title, "Lost connection");
            Dialog reconnect = ReflectionHelpers.getField(game, "streamMenu");
            assertNotNull(reconnect);
            assertTrue(reconnect.isShowing());
            assertTrue(game.menuActions.isTerminalDialog());

            game.onPause();
            ShadowLooper.idleMainLooper();
            assertSame("terminal reconnect dialog survives onPause", reconnect,
                    ReflectionHelpers.getField(game, "streamMenu"));
            assertTrue("terminal reconnect dialog remains showing across onPause", reconnect.isShowing());

            game.onResume();
            ShadowLooper.idleMainLooper();
            assertSame(reconnect, ReflectionHelpers.getField(game, "streamMenu"));
            assertTrue(reconnect.isShowing());

            // Finishing the activity still dismisses the terminal dialog cleanly in onPause.
            game.finish();
            game.onPause();
            ShadowLooper.idleMainLooper();
            assertFalse("finishing dismisses terminal dialog", reconnect.isShowing());
        } finally {
            cleanupRecovery(game);
        }

        // Also verify Moved prompt survives onPause() while non-terminal dialogs still dismiss.
        java.util.concurrent.atomic.AtomicInteger reads = new java.util.concurrent.atomic.AtomicInteger();
        Game moved = movedAwayFrom(pcAnswering(reads, STILL_MOVED));
        try {
            Dialog prompt = ReflectionHelpers.getField(moved, "streamMenu");
            assertNotNull(prompt);
            assertTrue(prompt.isShowing());
            moved.onPause();
            ShadowLooper.idleMainLooper();
            assertTrue("moved prompt remains showing across onPause", prompt.isShowing());
        } finally {
            cleanupRecovery(moved);
        }
    }

    @Test
    public void onPauseStillDismissesNonTerminalStreamDialogs() {
        Game game = establishedGame();
        try {
            Dialog regular = new NavigationDialogBuilder(game).setTitle("Menu").create();
            game.menuActions.showStreamDialog(regular);
            assertTrue(regular.isShowing());
            assertFalse(game.menuActions.isTerminalDialog());

            game.onPause();
            ShadowLooper.idleMainLooper();
            assertFalse("regular stream dialog must dismiss on onPause", regular.isShowing());
            assertNull(ReflectionHelpers.getField(game, "streamMenu"));
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    public void showStreamDialogDismissesPreviousDialogAndBalancesPipRefCount() {
        Game game = establishedGame();
        try {
            Dialog first = new NavigationDialogBuilder(game).setTitle("First").create();
            Dialog second = new NavigationDialogBuilder(game).setTitle("Second").create();

            game.menuActions.showStreamDialog(first);
            assertTrue(first.isShowing());
            assertEquals(1, game.suppressPipRefCount);

            game.menuActions.showStreamDialog(second);
            ShadowLooper.idleMainLooper();
            assertFalse("previous dialog must be dismissed", first.isShowing());
            assertTrue("replacement dialog must be showing", second.isShowing());
            assertSame(second, ReflectionHelpers.getField(game, "streamMenu"));
            assertEquals("suppressPipRefCount must stay balanced at 1", 1, game.suppressPipRefCount);

            second.dismiss();
            ShadowLooper.idleMainLooper();
            assertNull(ReflectionHelpers.getField(game, "streamMenu"));
            assertEquals("suppressPipRefCount returns to 0 after dismissal", 0, game.suppressPipRefCount);
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(qualifiers = "w640dp-h360dp-land")
    public void showVideoScalingDialogOnShortLandscapeScreenOffersAllFiveModesAndScrollsToSafeFit() {
        Game game = establishedGame();
        try {
            com.limelight.preferences.PreferenceConfiguration config = ReflectionHelpers.getField(game, "prefConfig");
            config.upscalingMode = com.limelight.binding.video.UpscalingPolicy.Mode.OFF;
            config.videoScaling = com.limelight.ui.VideoScaling.FIT;
            ReflectionHelpers.setField(game, "renderView", new com.limelight.ui.StreamView(game));

            game.videoOutput.showVideoScalingDialog();
            androidx.appcompat.app.AlertDialog dialog = ReflectionHelpers.getField(game, "streamMenu");
            assertNotNull("showVideoScalingDialog must open dialog", dialog);
            assertTrue(dialog.isShowing());

            android.widget.ListView list = dialog.getListView();
            assertNotNull(list);
            int expectedCount = game.getResources().getStringArray(R.array.video_scaling_modes).length;
            assertEquals(5, expectedCount);
            assertEquals(expectedCount, list.getCount());

            list.measure(android.view.View.MeasureSpec.makeMeasureSpec(640, android.view.View.MeasureSpec.EXACTLY),
                    android.view.View.MeasureSpec.makeMeasureSpec(220, android.view.View.MeasureSpec.EXACTLY));
            list.layout(0, 0, 640, 220);
            assertTrue("short landscape dialog list can scroll down to Safe fit",
                    list.canScrollVertically(1) || list.getLastVisiblePosition() == expectedCount - 1);

            list.setSelection(com.limelight.ui.VideoScaling.SAFE_FIT);
            list.performItemClick(list.getAdapter().getView(com.limelight.ui.VideoScaling.SAFE_FIT, null, list),
                    com.limelight.ui.VideoScaling.SAFE_FIT,
                    list.getAdapter().getItemId(com.limelight.ui.VideoScaling.SAFE_FIT));
            ShadowLooper.idleMainLooper();
            assertFalse("picking Safe fit closes dialog", dialog.isShowing());
            assertEquals(com.limelight.ui.VideoScaling.SAFE_FIT, config.videoScaling);
        } finally {
            cleanupRecovery(game);
        }
    }
}

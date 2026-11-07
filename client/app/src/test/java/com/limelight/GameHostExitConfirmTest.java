package com.limelight;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Activity;
import android.content.DialogInterface;
import android.widget.Button;

import androidx.appcompat.app.AlertDialog;
import androidx.core.content.ContextCompat;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.Shadows;
import org.robolectric.annotation.Config;

/**
 * The stream menu's Quit game confirm showed an error-coloured label on the moon: pale pink on amber, unreadable on a real
 * stream. Quit game, Close game and End session confirm in Home's red with its light label.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = android.app.Application.class, qualifiers = "w800dp-h400dp-land")
public class GameHostExitConfirmTest {
    private Activity activity;

    @Before
    public void theme() {
        activity = Robolectric.buildActivity(Activity.class).setup().get();
        activity.setTheme(R.style.AppTheme);
    }

    @Test
    public void everyHostExitConfirmIsTheRedOfHomesConfirms() {
        int[][] confirms = {
                {R.string.stream_quit_game, R.string.stream_quit_game_confirm},
                {R.string.stream_close_game, R.string.stream_close_game_confirm},
                {R.string.stream_end_session, R.string.stream_end_session_confirm},
        };
        for (int[] confirm : confirms) {
            AlertDialog dialog = GameMenuActions.hostExitDialog(activity, confirm[0], confirm[1], () -> { });
            dialog.show();
            Button action = dialog.getButton(DialogInterface.BUTTON_POSITIVE);
            String name = activity.getString(confirm[0]);
            assertEquals(name + ": red fill, not the moon", R.drawable.dialog_button_danger,
                    Shadows.shadowOf(action.getBackground()).getCreatedFromResId());
            assertEquals(name + ": the light label on the red", ContextCompat.getColor(activity, R.color.nj_color_on_danger_fill),
                    action.getCurrentTextColor());
            assertFalse(name + ": a pad starts on Cancel", action.isFocusedByDefault());
            assertTrue(dialog.getButton(DialogInterface.BUTTON_NEGATIVE).isFocusedByDefault());
            dialog.dismiss();
        }
    }

    @Test
    public void confirmingRunsTheExit() {
        boolean[] ran = {false};
        AlertDialog dialog = GameMenuActions.hostExitDialog(activity, R.string.stream_quit_game, R.string.stream_quit_game_confirm,
                () -> ran[0] = true);
        dialog.show();
        dialog.getButton(DialogInterface.BUTTON_POSITIVE).performClick();
        org.robolectric.shadows.ShadowLooper.idleMainLooper();
        assertTrue(ran[0]);
    }
}

package com.limelight;

import android.app.Application;
import android.content.Intent;
import android.view.View;
import android.widget.TextView;
import com.limelight.binding.input.HeldInputState;
import com.limelight.binding.input.TouchLayout;
import com.limelight.nvstream.http.NvApp;
import com.limelight.ui.GameTouchControls;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;
import java.util.Collections;
import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class GameTouchProfileTest {
    @Test public void resumeAndRunningGameSelectTheEstablishedGameAndKeepItAcrossRecovery() {
        for (int tileId : new int[] {2147483501, 2147483507}) {
            Game game = game();
            NvApp tile = new NvApp("Resume or Running Game", tileId, false);
            String tileProfile = TouchLayout.profileKey("pc", tile.getAppUuid(), tileId);
            String firstProfile = TouchLayout.profileKey("pc", "first-game", 73);
            String secondProfile = TouchLayout.profileKey("pc", "", 91);
            save(game, tileProfile, "Tile layout");
            save(game, firstProfile, "First game");
            save(game, secondProfile, "Second game");
            GameTouchControls controls = controls(game, tileProfile);
            ReflectionHelpers.setField(game, "app", tile);

            established(game, 73, "first-game");
            assertEquals("First game", label(controls));
            View selected = controls.getChildAt(1);
            NvApp recovered = new NvApp("First game", 73, false);
            recovered.setAppUuid("first-game");
            ReflectionHelpers.setField(game, "app", recovered);
            established(game, 73, "first-game");
            assertSame(selected, controls.getChildAt(1));

            // The same control tile can later refer to a different game, including an older host without UUIDs.
            ReflectionHelpers.setField(game, "app", tile);
            established(game, 91, "");
            assertEquals("Second game", label(controls));
            assertEquals("Tile layout", TouchLayout.load(game, tileProfile).buttons.get(0).name);
            assertEquals("First game", TouchLayout.load(game, firstProfile).buttons.get(0).name);
        }
    }

    @Test public void specialRolesAndUnresolvedSessionsRetainTheirSeparateControls() {
        NvApp monitorResume = new NvApp("Resume monitor", 2147483501, false);
        monitorResume.setMonitorResume(true);
        for (NvApp app : new NvApp[] {
                new NvApp("Remote Monitor", 2147483505, false),
                new NvApp("Input-only", 2147483506, false),
                new NvApp("Terminate", 2147483504, false), monitorResume,
                new NvApp("Unresolved resume", 2147483501, false)}) {
            Game game = game();
            save(game, "separate", "Separate controls");
            save(game, TouchLayout.profileKey("pc", "running-game", 73), "Game controls");
            GameTouchControls controls = controls(game, "separate");
            View selected = controls.getChildAt(1);
            ReflectionHelpers.setField(game, "app", app);
            established(game, app.getAppName().equals("Unresolved resume") ? 0 : 73, "running-game");
            assertSame(selected, controls.getChildAt(1));
            assertEquals("Separate controls", label(controls));
        }
    }

    private static Game game() {
        Game game = Robolectric.buildActivity(Game.class).get();
        game.setTheme(R.style.AppTheme);
        game.setIntent(new Intent().putExtra(Game.EXTRA_PC_UUID, "pc"));
        return game;
    }

    private static void save(Game game, String profile, String name) {
        new TouchLayout(Collections.singletonList(new TouchLayout.Button("hold", name, new int[0], 1,
                false, .1f, .1f, .2f, .2f))).save(game, profile);
        TouchLayout.setEnabled(game, profile, true);
    }

    private static GameTouchControls controls(Game game, String profile) {
        GameTouchControls controls = new GameTouchControls(game, profile, new HeldInputState.Sink() {
            public void key(int key, boolean down) { fail("Selecting a profile must not press keys"); }
            public void mouse(int button, boolean down) { fail("Selecting a profile must not press mouse buttons"); }
        }, () -> true, dialog -> { });
        ReflectionHelpers.setField(game, "gameTouchControls", controls);
        return controls;
    }

    private static void established(Game game, int id, String uuid) {
        ReflectionHelpers.setField(game, "recoveryAppId", id);
        ReflectionHelpers.setField(game, "recoveryAppUuid", uuid);
        ReflectionHelpers.callInstanceMethod(game, "updateGameTouchProfile");
    }

    private static String label(GameTouchControls controls) {
        // Child 0 forwards touches beside the controls to the stream; the first control follows it.
        return ((TextView) controls.getChildAt(1)).getText().toString();
    }
}

package com.limelight;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.ActivityInfo;
import android.content.pm.PackageManager;

import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.annotation.Config;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.DirectoryStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.List;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class DualScreenTest {
    @Test
    public void theSecondScreenIsTheFirstUsableDisplayBesideTheMainOne() {
        assertEquals(2, DualScreen.pickSecondDisplayId(new int[] {0, 1, 2}, new boolean[] {true, false, true}, 0));
        assertEquals(1, DualScreen.pickSecondDisplayId(new int[] {0, 1, 2}, new boolean[] {true, true, true}, 0));
    }

    @Test
    public void aDeviceWithOneScreenHasNoSecondScreen() {
        assertEquals(-1, DualScreen.pickSecondDisplayId(new int[] {0}, new boolean[] {true}, 0));
        assertEquals(-1, DualScreen.pickSecondDisplayId(new int[] {0, 3}, new boolean[] {true, false}, 0));
        assertEquals(-1, DualScreen.pickSecondDisplayId(new int[0], new boolean[0], 0));
    }

    @Test
    public void aStreamNotOnTheDefaultDisplayLeavesThatOneAsTheOther() {
        assertEquals(0, DualScreen.pickSecondDisplayId(new int[] {0, 1}, new boolean[] {true, true}, 1));
    }

    @Test
    public void theSecondScreenTakesAFixedSizeOrItsOwn() {
        PreferenceConfiguration config = new PreferenceConfiguration();
        assertTrue(config.useResolution("1280x720"));
        assertEquals(1280, config.width);
        assertEquals(720, config.height);
        assertFalse(config.resolutionFollowsScreen);
        assertFalse(config.useResolution(PreferenceConfiguration.SECOND_SCREEN_NATIVE));
        assertFalse(config.useResolution("0x720"));
        assertFalse(config.useResolution(null));
        // Digits alone are no size: a zero or a giant side keeps the screen's own size.
        assertFalse(config.useResolution("00x720"));
        assertFalse(config.useResolution("1280x00"));
        assertFalse(config.useResolution("32x32"));
        assertFalse(config.useResolution("99999x1080"));
        assertEquals(1280, config.width);
        assertEquals(720, config.height);
        assertTrue(config.useResolution("3840x2160"));
        assertEquals(2160, config.height);
    }

    @Test
    public void aSecondFollowsOnlyTheEndOfItsOwnFirst() {
        Intent second = new Intent().putExtra(DualScreen.EXTRA_SECOND_SCREEN, true)
                .putExtra(DualScreen.EXTRA_FIRST_STREAM, "a");
        assertTrue(DualScreen.followsEnd(second, new Intent(DualScreen.ACTION_END)
                .putExtra(DualScreen.EXTRA_FIRST_STREAM, "a")));
        // A first that reconnected or switched games started this second again; its old end is not this one's.
        assertFalse(DualScreen.followsEnd(second, new Intent(DualScreen.ACTION_END)
                .putExtra(DualScreen.EXTRA_FIRST_STREAM, "b")));
        assertFalse(DualScreen.followsEnd(second, new Intent(DualScreen.ACTION_END)));
        assertFalse(DualScreen.followsEnd(new Intent().putExtra(DualScreen.EXTRA_FIRST_STREAM, "a"),
                new Intent(DualScreen.ACTION_END).putExtra(DualScreen.EXTRA_FIRST_STREAM, "a")));
    }

    @Test
    public void aFirstThatGoesWithoutFinishingStillEndsItsSecond() {
        Game game = Robolectric.buildActivity(Game.class).get();
        game.dualScreenStarted = true;
        game.dualScreenFirst = "a";
        assertFalse(game.isFinishing());
        DualScreen.end(game);
        List<Intent> sent = Shadows.shadowOf(RuntimeEnvironment.getApplication()).getBroadcastIntents();
        assertEquals(1, sent.size());
        assertEquals(DualScreen.ACTION_END, sent.get(0).getAction());
        assertEquals("a", sent.get(0).getStringExtra(DualScreen.EXTRA_FIRST_STREAM));
    }

    @Test
    public void theSecondEndsNothingWhenItGoes() {
        Game game = Robolectric.buildActivity(Game.class,
                new Intent().putExtra(DualScreen.EXTRA_SECOND_SCREEN, true)).get();
        game.dualScreenStarted = true;
        DualScreen.end(game);
        assertTrue(Shadows.shadowOf(RuntimeEnvironment.getApplication()).getBroadcastIntents().isEmpty());
    }

    @Test
    public void theSecondProcessEndsOnlyAfterItsStreamStopped() {
        assertEquals(GameSecondary.ProcessEnd.STAY, GameSecondary.processEnd(1, false, 60_000));
        assertEquals(GameSecondary.ProcessEnd.WAIT, GameSecondary.processEnd(0, false, 1000));
        assertEquals(GameSecondary.ProcessEnd.END, GameSecondary.processEnd(0, false, GameSecondary.PROCESS_LINGER_MS));
        // The stop still tells the host the stream is over; the process waits for it.
        assertEquals(GameSecondary.ProcessEnd.WAIT, GameSecondary.processEnd(0, true, GameSecondary.PROCESS_LINGER_MS));
        assertEquals(GameSecondary.ProcessEnd.END, GameSecondary.processEnd(0, true, GameConnection.STOP_REPORT_MS));
    }

    /** A stream menu change on the first screen is never undone by the second screen's old copy of the settings. */
    @Test
    public void theSecondScreenSavesNoSettings() {
        SharedPreferences prefs = RuntimeEnvironment.getApplication().getSharedPreferences("dual", Context.MODE_PRIVATE);
        Intent second = new Intent().putExtra(DualScreen.EXTRA_SECOND_SCREEN, true);
        assertFalse(DualScreen.saveSettings(prefs, second, editor -> editor.putBoolean("overlay", true)));
        assertFalse(prefs.contains("overlay"));
        assertTrue(DualScreen.saveSettings(prefs, new Intent(), editor -> editor.putBoolean("overlay", true)));
        assertTrue(prefs.getBoolean("overlay", false));
    }

    /** Every settings write of the stream's parts goes through {@link DualScreen#saveSettings}. */
    @Test
    public void theStreamWritesSettingsOnlyThroughTheGuard() throws IOException {
        Path sources = Paths.get("src/main/java/com/limelight");
        Pattern edit = Pattern.compile("(\\w+)\\s*\\)?\\s*\\.edit\\(\\)");
        int files = 0;
        try (DirectoryStream<Path> stream = Files.newDirectoryStream(sources, "Game*.java")) {
            for (Path file : stream) {
                files++;
                String text = new String(Files.readAllBytes(file), StandardCharsets.UTF_8);
                Matcher m = edit.matcher(text);
                while (m.find()) {
                    // The crash counter is the process's own bookkeeping, not a setting.
                    assertEquals(file + " writes settings past DualScreen.saveSettings", "tombstonePrefs", m.group(1));
                }
            }
        }
        assertTrue(files > 5);
    }

    /** No other app can end a stream or change its touch mode: the link takes broadcasts from this app only. */
    @Test
    public void theLinkTakesBroadcastsFromThisAppOnly() {
        Game game = Robolectric.buildActivity(Game.class).get();
        // Declared by androidx core in the merged manifest and held by this app; Robolectric grants nothing itself.
        String permission = game.getPackageName() + ".DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION";
        Shadows.shadowOf(RuntimeEnvironment.getApplication()).grantPermissions(permission);
        DualScreen.link(game);
        boolean found = false;
        for (org.robolectric.shadows.ShadowApplication.Wrapper wrapper
                : Shadows.shadowOf(RuntimeEnvironment.getApplication()).getRegisteredReceivers()) {
            if (wrapper.broadcastReceiver == game.dualScreenLink) {
                found = true;
                // Below Android 13 ContextCompat guards it with a permission only this app holds.
                assertEquals(permission, wrapper.broadcastPermission);
            }
        }
        assertTrue(found);
    }

    /** The second screen trusts its intent (and asks for no PIN) because only this app can start it. */
    @Test
    public void onlyThisAppCanStartTheSecondScreen() throws PackageManager.NameNotFoundException {
        Application app = RuntimeEnvironment.getApplication();
        ActivityInfo info = app.getPackageManager().getActivityInfo(new ComponentName(app, GameSecondary.class), 0);
        assertFalse(info.exported);
    }

    @Test
    public void onlyAnIntentMarkedAsTheSecondStreamIsOne() {
        assertFalse(DualScreen.isSecond(null));
        assertFalse(DualScreen.isSecond(new Intent()));
        assertTrue(DualScreen.isSecond(new Intent().putExtra(DualScreen.EXTRA_SECOND_SCREEN, true)));
    }
}

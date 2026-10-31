package com.limelight;

import android.app.Application;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.os.Looper;
import android.widget.Toast;

import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.TestHosts;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowToast;
import org.robolectric.util.ReflectionHelpers;

import java.io.File;
import java.io.IOException;
import java.io.OutputStream;
import java.util.concurrent.TimeUnit;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

/** Clipboard copies from the host land without window focus, and a stream that closed meanwhile is told, not silent. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class GameClipboardTest {
    private static final byte[] PNG = {(byte) 0x89, 'P', 'N', 'G', '\r', '\n', 0x1a, '\n', 0, 0, 0, 13,
            'I', 'H', 'D', 'R', 0, 0, 0, 2, 0, 0, 0, 2, 8, 6, 0, 0, 0};

    private Game game(Runnable onFetch) throws Exception {
        Game game = Robolectric.buildActivity(Game.class).get();
        game.setTheme(R.style.StreamTheme);
        NvApp app = new NvApp("Desktop", 73, false);
        ReflectionHelpers.setField(game, "app", app);
        ReflectionHelpers.setField(game, "foreground", true);
        ReflectionHelpers.setField(game, "connected", true);
        ComputerDetails details = TestHosts.details("rubylight-serverinfo.xml");
        NvConnection connection = new NvConnection(null, new ComputerDetails.AddressTuple("192.0.2.1", 47989),
                47984, "test", new StreamConfiguration.Builder().setApp(app).build(), null, null) {
            @Override public void stop() {}
            @Override public String getClipboard() {
                onFetch.run();
                return "from the host";
            }
            @Override public boolean getClipboardImage(OutputStream out) throws IOException {
                out.write(PNG);
                onFetch.run();
                return true;
            }
        };
        ReflectionHelpers.setField(connection, "hostDetails", details);
        ReflectionHelpers.setField(game, "conn", connection);
        return game;
    }

    /** Runs the worker's posts until the host action is over. */
    private static void settle(Game game) throws InterruptedException {
        long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(5);
        do {
            Thread.sleep(5);
            shadowOf(Looper.getMainLooper()).idle();
        } while ((boolean) ReflectionHelpers.getField(game, "hostActionInProgress") && System.nanoTime() < deadline);
        assertFalse((boolean) ReflectionHelpers.getField(game, "hostActionInProgress"));
    }

    private static File[] incoming(Game game) {
        File[] files = new File(game.getCacheDir(), "clipboard").listFiles();
        return files == null ? new File[0] : files;
    }

    @Test
    public void hostTextIsCopiedWhileTheWindowHasNoFocus() throws Exception {
        Game game = game(() -> {});
        // An unattached activity has no window focus, as while a toast or the stream menu's dialog holds it.
        assertFalse(game.hasWindowFocus());
        ReflectionHelpers.callInstanceMethod(game.menuActions, "transferClipboard",
                ReflectionHelpers.ClassParameter.from(boolean.class, false));
        settle(game);
        ClipboardManager clipboard = (ClipboardManager) game.getSystemService(Context.CLIPBOARD_SERVICE);
        ClipData clip = clipboard.getPrimaryClip();
        assertNotNull(clip);
        assertEquals("from the host", clip.getItemAt(0).getText().toString());
        assertEquals(game.getString(R.string.stream_clipboard_received), ShadowToast.getTextOfLatestToast());
    }

    @Test
    public void hostTextForAStreamThatClosedIsToldNotDropped() throws Exception {
        Game[] holder = new Game[1];
        Game game = game(() -> ReflectionHelpers.setField(holder[0], "connected", false));
        holder[0] = game;
        ReflectionHelpers.callInstanceMethod(game.menuActions, "transferClipboard",
                ReflectionHelpers.ClassParameter.from(boolean.class, false));
        settle(game);
        ClipboardManager clipboard = (ClipboardManager) game.getSystemService(Context.CLIPBOARD_SERVICE);
        assertFalse(clipboard.hasPrimaryClip());
        assertEquals(game.getString(R.string.stream_clipboard_not_copied), ShadowToast.getTextOfLatestToast());
        assertEquals(Toast.LENGTH_LONG, ShadowToast.getLatestToast().getDuration());
    }

    @Test
    public void aHostImageForAStreamThatClosedIsToldAndItsFileGoes() throws Exception {
        Game[] holder = new Game[1];
        Game game = game(() -> ReflectionHelpers.setField(holder[0], "connected", false));
        holder[0] = game;
        File directory = new File(game.getCacheDir(), "clipboard");
        assertTrue(directory.isDirectory() || directory.mkdirs());
        File before = File.createTempFile("clipboard-", ".png", directory);
        ReflectionHelpers.callInstanceMethod(game.menuActions, "transferClipboardImage",
                ReflectionHelpers.ClassParameter.from(boolean.class, false));
        settle(game);
        assertEquals(game.getString(R.string.stream_clipboard_image_not_copied), ShadowToast.getTextOfLatestToast());
        // The image already on the clipboard keeps its file; the one that never got there is gone.
        File[] left = incoming(game);
        assertEquals(1, left.length);
        assertEquals(before, left[0]);
    }
}

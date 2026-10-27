package com.limelight;

import android.app.Application;
import android.os.Looper;

import androidx.appcompat.app.AlertDialog;

import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.TestHosts;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.StreamMenu;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.util.ReflectionHelpers;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

/** The real menu, confirmation and NvConnection permission/identity checks, up to JNI. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, shadows = GameServerCommandTest.Commands.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameServerCommandTest {
    @Implements(value = MoonBridge.class, isInAndroidSdk = false)
    public static class Commands {
        static volatile byte[] payload;
        static final java.util.concurrent.atomic.AtomicInteger calls = new java.util.concurrent.atomic.AtomicInteger();
        static CountDownLatch sent;
        @Implementation protected static void __staticInitializer__() {}
        @Implementation protected static boolean sendServerCommand(byte[] bytes) {
            payload = bytes;
            calls.incrementAndGet();
            sent.countDown();
            return true;
        }
    }

    private Game game(ComputerDetails details) {
        Game game = Robolectric.buildActivity(Game.class).get();
        game.setTheme(R.style.StreamTheme);
        game.setContentView(R.layout.activity_game);
        NvApp app = new NvApp("Desktop", 73, false);
        ReflectionHelpers.setField(game, "app", app);
        ReflectionHelpers.setField(game, "prefConfig", new PreferenceConfiguration());
        ReflectionHelpers.setField(game, "foreground", true);
        ReflectionHelpers.setField(game, "connected", true);
        NvConnection connection = new NvConnection(null, new ComputerDetails.AddressTuple("192.0.2.1", 47989),
                47984, "test", new StreamConfiguration.Builder().setApp(app).build(), null, null) {
            @Override public void stop() {}
            @Override public ComputerDetails refreshHostDetails() { return details; }
        };
        ReflectionHelpers.setField(connection, "hostDetails", details);
        ReflectionHelpers.setField(connection, "nativeConnected", true);
        ReflectionHelpers.setField(game, "conn", connection);
        return game;
    }

    private AlertDialog sleepConfirmation(Game game, int index) {
        game.showStreamMenu();
        StreamMenu menu = ReflectionHelpers.getField(game, "streamMenuSheet");
        menu.row(StreamMenu.Item.SERVER_COMMANDS).performClick();
        shadowOf(Looper.getMainLooper()).idleFor(WindowFocusActionQueue.TIMEOUT_MS, TimeUnit.MILLISECONDS);
        // This lightweight Game has no attached stream window to deliver a focus callback.
        ((WindowFocusActionQueue) ReflectionHelpers.getField(game, "windowFocusActions"))
                .runPending(true, android.os.SystemClock.uptimeMillis());
        AlertDialog picker = ReflectionHelpers.getField(game, "streamMenu");
        assertTrue(picker.isShowing());
        picker.getListView().performItemClick(null, index, index);
        shadowOf(Looper.getMainLooper()).idle();
        AlertDialog confirmation = ReflectionHelpers.getField(game, "streamMenu");
        assertNotSame(picker, confirmation);
        assertTrue(confirmation.isShowing());
        return confirmation;
    }

    @Test
    public void sleepIsSentOnceOnlyAfterConfirmationWithTheHostsIndexAndFormat() throws Exception {
        for (String fixture : new String[] {"apollo-serverinfo.xml", "vibepollo-serverinfo.xml", "rubylight-serverinfo.xml"}) {
            ComputerDetails details = TestHosts.details(fixture);
            int index = fixture.startsWith("apollo") ? 2 : 0;
            details.serverCommands.set(index, "Sleep");
            Commands.payload = null;
            Commands.calls.set(0);
            Commands.sent = new CountDownLatch(1);
            Game game = game(details);
            AlertDialog cancel = sleepConfirmation(game, index);
            assertNull(Commands.payload);
            cancel.getButton(AlertDialog.BUTTON_NEGATIVE).performClick();
            shadowOf(Looper.getMainLooper()).idleFor(WindowFocusActionQueue.TIMEOUT_MS, TimeUnit.MILLISECONDS);
            assertNull("Cancel must not send Sleep", Commands.payload);
            AlertDialog confirm = sleepConfirmation(game, index);
            confirm.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            shadowOf(Looper.getMainLooper()).idle();
            assertTrue("Sleep did not reach JNI for " + fixture, Commands.sent.await(2, TimeUnit.SECONDS));
            assertArrayEquals(details.serverCommandPayload(index), Commands.payload);
            // Let the worker finish posting its result and clear the busy state before teardown.
            long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(2);
            while ((boolean) ReflectionHelpers.getField(game, "hostActionInProgress") && System.nanoTime() < deadline) {
                Thread.sleep(5);
                shadowOf(Looper.getMainLooper()).idle();
            }
            assertFalse((boolean) ReflectionHelpers.getField(game, "hostActionInProgress"));
            assertEquals(1, Commands.calls.get());
        }
    }
}

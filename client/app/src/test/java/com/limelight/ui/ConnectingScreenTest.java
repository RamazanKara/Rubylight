package com.limelight.ui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Activity;
import android.view.View;
import android.widget.TextView;

import androidx.appcompat.app.AppCompatActivity;

import com.limelight.R;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;

import java.util.concurrent.atomic.AtomicInteger;

/** Connecting and a failed connection: the words follow the stage, the window goes when it is done, every way out works. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34)
public class ConnectingScreenTest {
    public static class Stream extends AppCompatActivity {
        @Override protected void onCreate(android.os.Bundle state) {
            setTheme(R.style.StreamTheme);
            super.onCreate(state);
        }
    }

    private static String text(android.app.Dialog dialog, int id) {
        return ((TextView) dialog.findViewById(id)).getText().toString();
    }

    @Test
    public void theWordFollowsTheStageAndTheWindowGoesWhenDone() {
        ActivityController<Stream> controller = Robolectric.buildActivity(Stream.class).setup();
        AtomicInteger cancelled = new AtomicInteger();
        ConnectingScreen screen = ConnectingScreen.show(controller.get(), "Racing Game", "Living-room PC", "Connecting", "Starting connection", cancelled::incrementAndGet);
        android.app.Dialog dialog = screen.dialog();
        ShadowLooper.idleMainLooper();
        assertTrue(dialog.isShowing());
        assertEquals("Racing Game", text(dialog, R.id.conn_title));
        assertEquals("Living-room PC", text(dialog, R.id.conn_pc));
        assertEquals("Connecting", text(dialog, R.id.conn_word));
        assertEquals("Starting connection", text(dialog, R.id.conn_line));
        assertTrue("the orbit moves while it connects", screen.isOrbitMoving());

        screen.setStage("Starting");
        ShadowLooper.idleMainLooper();
        assertEquals("Starting", text(dialog, R.id.conn_word));
        assertEquals(View.GONE, dialog.findViewById(R.id.conn_line).getVisibility());

        screen.dismiss();
        ShadowLooper.idleMainLooper();
        assertFalse("the window is removed, not hidden, so the stream has nothing over it", dialog.isShowing());
        assertFalse(screen.isOrbitMoving());
        assertEquals(0, cancelled.get());
        controller.pause().stop().destroy();
    }

    @Test
    public void cancelEndsItAsBackDoes() {
        ActivityController<Stream> controller = Robolectric.buildActivity(Stream.class).setup();
        AtomicInteger cancelled = new AtomicInteger();
        ConnectingScreen screen = ConnectingScreen.show(controller.get(), "Racing Game", "PC", "Connecting", null, cancelled::incrementAndGet);
        assertEquals(View.GONE, screen.dialog().findViewById(R.id.conn_line).getVisibility());
        screen.dialog().findViewById(R.id.conn_cancel).performClick();
        ShadowLooper.idleMainLooper();
        assertEquals(1, cancelled.get());
        assertFalse(screen.dialog().isShowing());
        controller.pause().stop().destroy();
    }

    @Test
    public void aFinishingActivityOpensNothing() {
        ActivityController<Stream> controller = Robolectric.buildActivity(Stream.class).setup();
        Activity activity = controller.get();
        activity.finish();
        assertNull(ConnectingScreen.show(activity, "Racing Game", "PC", "Connecting", null, () -> { }));
        controller.pause().stop().destroy();
    }

    @Test
    public void aFailureSaysAllOfItAndOffersTryAgainAndTheNetworkTest() throws Exception {
        ActivityController<Stream> controller = Robolectric.buildActivity(Stream.class).setup();
        AtomicInteger tried = new AtomicInteger();
        AtomicInteger back = new AtomicInteger();
        String message = "Stream failed to start.\nClose protected videos or restart the PC.\n\nError code: 5031";
        ConnectingScreen screen = ConnectingScreen.failure(controller.get(), "Racing Game", "Living-room PC", null, "Connection Error", message,
                new ConnectingScreen.Failure() {
                    @Override public void onTryAgain() {
                        tried.incrementAndGet();
                    }

                    @Override public CharSequence testNetwork() {
                        return "Blocked";
                    }

                    @Override public void onBack() {
                        back.incrementAndGet();
                    }
                });
        screen.showFailure();
        android.app.Dialog dialog = screen.dialog();
        ShadowLooper.idleMainLooper();
        assertEquals("Connection Error", text(dialog, R.id.conn_error_title));
        assertEquals("the app's real message, unshortened", message, text(dialog, R.id.conn_error_message));
        assertEquals("Racing Game, Living-room PC", text(dialog, R.id.conn_error_what));
        assertEquals(View.GONE, dialog.findViewById(R.id.conn_cancel).getVisibility());
        assertFalse("nothing is under way", screen.isOrbitMoving());
        assertEquals(dialog.getContext().getString(R.string.pair_try_again), text(dialog, R.id.conn_retry));
        assertEquals("Test the network", text(dialog, R.id.conn_test));

        View test = dialog.findViewById(R.id.conn_test);
        test.performClick();
        for (int i = 0; i < 200 && !test.isEnabled(); i++) {
            Thread.sleep(5);
            ShadowLooper.idleMainLooper();
        }
        assertTrue(test.isEnabled());
        assertEquals("Blocked", text(dialog, R.id.conn_test_result));
        assertEquals(View.VISIBLE, dialog.findViewById(R.id.conn_test_result).getVisibility());
        assertNotNull(screen);

        dialog.findViewById(R.id.conn_retry).performClick();
        ShadowLooper.idleMainLooper();
        assertEquals(1, tried.get());
        assertEquals(0, back.get());
        assertFalse(dialog.isShowing());
        controller.pause().stop().destroy();
    }

    @Test
    public void backLeavesAFailure() {
        ActivityController<Stream> controller = Robolectric.buildActivity(Stream.class).setup();
        AtomicInteger back = new AtomicInteger();
        ConnectingScreen screen = ConnectingScreen.failure(controller.get(), "Racing Game", "PC", null, "Connection Error", "No.",
                new ConnectingScreen.Failure() {
                    @Override public void onTryAgain() { }

                    @Override public CharSequence testNetwork() {
                        return "";
                    }

                    @Override public void onBack() {
                        back.incrementAndGet();
                    }
                });
        screen.showFailure();
        screen.dialog().findViewById(R.id.conn_back).performClick();
        ShadowLooper.idleMainLooper();
        assertEquals(1, back.get());
        controller.pause().stop().destroy();
    }
}

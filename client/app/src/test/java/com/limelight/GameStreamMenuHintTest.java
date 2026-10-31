package com.limelight;

import android.app.Application;
import android.view.View;
import android.widget.TextView;

import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.TestHosts;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.StreamMenuHint;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.time.Duration;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameStreamMenuHintTest {
    private static Game streamingGame() {
        Game game = Robolectric.buildActivity(Game.class).get();
        game.setTheme(R.style.StreamTheme);
        game.setContentView(R.layout.activity_game);
        ReflectionHelpers.setField(game, "prefConfig", new PreferenceConfiguration());
        NvApp app = new NvApp("Game", 73, false);
        ReflectionHelpers.setField(game, "app", app);
        ReflectionHelpers.setField(game, "foreground", true);
        ReflectionHelpers.setField(game, "connected", true);
        ReflectionHelpers.setField(game, "performanceOverlayView", game.findViewById(R.id.performanceOverlay));
        ReflectionHelpers.setField(game, "performanceOverlayScroll", game.findViewById(R.id.performanceOverlayScroll));
        ReflectionHelpers.setField(game, "streamMenuHint", game.findViewById(R.id.streamMenuHint));
        NvConnection connection = new NvConnection(null, new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984,
                "test", new StreamConfiguration.Builder().setApp(app).build(), null, null) {
            @Override public void stop() {}
        };
        ReflectionHelpers.setField(connection, "hostDetails", TestHosts.details("rubylight-serverinfo.xml"));
        ReflectionHelpers.setField(game, "conn", connection);
        return game;
    }

    private static TextView hint(Game game) {
        return game.findViewById(R.id.streamMenuHint);
    }

    private static void show(Game game) {
        ReflectionHelpers.callInstanceMethod(game.overlays, "showStreamMenuHint");
    }

    @Test
    public void theFirstStreamSaysHowToOpenTheMenuOnceAndATapClosesIt() {
        Game game = streamingGame();
        show(game);
        assertEquals(View.VISIBLE, hint(game).getVisibility());
        assertTrue(hint(game).getText().toString().startsWith("To open the stream menu, "));
        assertTrue(hint(game).getContentDescription().toString().endsWith("Tap to close"));
        hint(game).performClick();
        assertEquals(View.GONE, hint(game).getVisibility());
        assertTrue(StreamMenuHint.seen(game));

        Game next = streamingGame();
        show(next);
        assertEquals("Shown once", View.GONE, hint(next).getVisibility());
    }

    @Test
    public void unreadItGoesAfterAWhileAndCountsAsSeen() {
        Game game = streamingGame();
        show(game);
        shadowOf(android.os.Looper.getMainLooper()).idleFor(Duration.ofMillis(StreamMenuHint.SHOW_MS + 1));
        assertEquals(View.GONE, hint(game).getVisibility());
        assertTrue(StreamMenuHint.seen(game));
    }

    @Test
    public void openingTheMenuRetiresTheHint() {
        Game game = streamingGame();
        show(game);
        game.showStreamMenu();
        assertEquals(View.GONE, hint(game).getVisibility());
        assertTrue(StreamMenuHint.seen(game));
        android.app.Dialog menu = ReflectionHelpers.getField(game, "streamMenu");
        menu.dismiss();
    }

    @Test
    public void leavingBeforeItWasReadShowsItAgainNextTime() {
        Game game = streamingGame();
        show(game);
        ReflectionHelpers.callInstanceMethod(game.overlays, "dismissStreamMenuHint",
                ReflectionHelpers.ClassParameter.from(boolean.class, false));
        assertEquals(View.GONE, hint(game).getVisibility());
        assertFalse(StreamMenuHint.seen(game));
        Game next = streamingGame();
        show(next);
        assertEquals(View.VISIBLE, hint(next).getVisibility());
    }
}

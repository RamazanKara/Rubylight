package com.limelight;

import android.app.Application;
import android.content.ClipboardManager;
import android.content.Context;
import android.preference.PreferenceManager;
import android.widget.TextView;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GamePerformanceOverlayTest {
    private Game game() {
        Game game = Robolectric.buildActivity(Game.class).get();
        game.setContentView(R.layout.activity_game);
        ReflectionHelpers.setField(game, "performanceOverlayView", game.findViewById(R.id.performanceOverlay));
        ReflectionHelpers.setField(game, "performanceOverlayScroll", game.findViewById(R.id.performanceOverlayScroll));
        ReflectionHelpers.setField(game, "statisticsPerformanceText", "1920×1080 · AV1 · HDR\nDisplayed: 60.0 FPS\nVideo payload: 10.0 Mbps\nNetwork RTT: 8 ms\nHost processing: — ms\nDecode: 4.0 ms");
        ReflectionHelpers.setField(game, "debugPerformanceText", "Video\nAV1\n\nDecode\nDriver fallback\np95/p99: 5/6 ms");
        return game;
    }

    @Test
    public void switchingModesKeepsAllLinesAndPersistsTheNamedMode() {
        Game game = game();
        TextView overlay = game.findViewById(R.id.performanceOverlay);
        ReflectionHelpers.callInstanceMethod(game, "updatePerformanceOverlay");
        assertEquals(6, overlay.getText().toString().split("\n").length);
        assertNotEquals(1, overlay.getMaxLines());
        assertNull(overlay.getEllipsize());
        ReflectionHelpers.callInstanceMethod(game, "togglePerformanceOverlayMode");
        assertTrue(overlay.getText().toString().contains("Driver fallback"));
        assertEquals("debug", PreferenceManager.getDefaultSharedPreferences(game).getString("performance_overlay_mode", ""));
        ReflectionHelpers.callInstanceMethod(game, "togglePerformanceOverlayMode");
        assertTrue(overlay.getText().toString().startsWith("1920×1080"));
        assertEquals("statistics", PreferenceManager.getDefaultSharedPreferences(game).getString("performance_overlay_mode", ""));
    }

    @Test
    public void copyingFromStatisticsKeepsTheFullMultilineDebugReport() {
        Game game = game();
        ReflectionHelpers.callInstanceMethod(game, "copyPerformanceStats");
        ClipboardManager clipboard = (ClipboardManager) game.getSystemService(Context.CLIPBOARD_SERVICE);
        assertEquals("Video\nAV1\n\nDecode\nDriver fallback\np95/p99: 5/6 ms", clipboard.getPrimaryClip().getItemAt(0).getText().toString());
    }
}

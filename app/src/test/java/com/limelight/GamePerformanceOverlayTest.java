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

    private static void setMode(Game game, String mode) {
        ReflectionHelpers.callInstanceMethod(game, "setPerformanceOverlayMode",
                ReflectionHelpers.ClassParameter.from(String.class, mode));
    }

    @Test
    public void compactIsTheDefaultOneLineWithItsDotInWords() {
        Game game = game();
        TextView overlay = game.findViewById(R.id.performanceOverlay);
        ReflectionHelpers.callInstanceMethod(game, "updatePerformanceOverlay");
        assertEquals("● Waiting for video", overlay.getText().toString());
        ReflectionHelpers.setField(game, "compactPerformanceText", com.limelight.binding.video.PerformanceOverlay
                .compactText(game, 119.94f, 120, 8, 0.14f, 4.6f));
        ReflectionHelpers.callInstanceMethod(game, "updatePerformanceOverlay");
        assertEquals("● 119.9 FPS · 8 ms RTT · 0.1% loss", overlay.getText().toString());
        assertEquals("Stream good: 119.9 FPS · 8 ms RTT · 0.1% loss", overlay.getContentDescription().toString());
    }

    @Test
    public void switchingModesKeepsAllLinesAndPersistsTheNamedMode() {
        Game game = game();
        TextView overlay = game.findViewById(R.id.performanceOverlay);
        setMode(game, "statistics");
        assertEquals(6, overlay.getText().toString().split("\n").length);
        assertNotEquals(1, overlay.getMaxLines());
        assertNull(overlay.getEllipsize());
        assertNull(overlay.getContentDescription());
        assertEquals("statistics", PreferenceManager.getDefaultSharedPreferences(game).getString("performance_overlay_mode", ""));
        setMode(game, "debug");
        assertTrue(overlay.getText().toString().contains("Driver fallback"));
        assertEquals("debug", PreferenceManager.getDefaultSharedPreferences(game).getString("performance_overlay_mode", ""));
        setMode(game, "compact");
        assertEquals("compact", PreferenceManager.getDefaultSharedPreferences(game).getString("performance_overlay_mode", ""));
        assertFalse(overlay.getText().toString().contains("\n"));
    }

    @Test
    public void aLoweredBitrateShowsInEveryModeUntilTheHostIsBackAtFullRate() {
        Game game = game();
        TextView overlay = game.findViewById(R.id.performanceOverlay);
        String notice = "Host lowered the bitrate to 38 Mbps: network";
        ReflectionHelpers.setField(game, "compactPerformanceText", com.limelight.binding.video.PerformanceOverlay
                .compactText(game, 119.94f, 120, 8, 0.14f, 4.6f));
        game.linkNotice(1, 150_000, 37_500);
        org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
        assertEquals("● 119.9 FPS · 8 ms RTT · 0.1% loss\n" + notice, overlay.getText().toString());
        assertEquals("Stream good: 119.9 FPS · 8 ms RTT · 0.1% loss. " + notice,
                overlay.getContentDescription().toString());
        setMode(game, "statistics");
        assertTrue(overlay.getText().toString().endsWith("Decode: 4.0 ms\n" + notice));
        setMode(game, "debug");
        assertTrue(overlay.getText().toString().endsWith("p95/p99: 5/6 ms\n" + notice));
        // The copied report is the measurements, not the notice.
        ReflectionHelpers.callInstanceMethod(game, "copyPerformanceStats");
        ClipboardManager clipboard = (ClipboardManager) game.getSystemService(Context.CLIPBOARD_SERVICE);
        assertEquals("Video\nAV1\n\nDecode\nDriver fallback\np95/p99: 5/6 ms",
                clipboard.getPrimaryClip().getItemAt(0).getText().toString());
        game.linkNotice(0, 150_000, 150_000);
        org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
        assertTrue(overlay.getText().toString().endsWith("p95/p99: 5/6 ms"));
        setMode(game, "compact");
        assertEquals("● 119.9 FPS · 8 ms RTT · 0.1% loss", overlay.getText().toString());
        assertEquals("Stream good: 119.9 FPS · 8 ms RTT · 0.1% loss", overlay.getContentDescription().toString());
    }

    @Test
    public void copyingFromStatisticsKeepsTheFullMultilineDebugReport() {
        Game game = game();
        ReflectionHelpers.callInstanceMethod(game, "copyPerformanceStats");
        ClipboardManager clipboard = (ClipboardManager) game.getSystemService(Context.CLIPBOARD_SERVICE);
        assertEquals("Video\nAV1\n\nDecode\nDriver fallback\np95/p99: 5/6 ms", clipboard.getPrimaryClip().getItemAt(0).getText().toString());
    }

    @Test
    public void statisticsKeepTheStreamsLastingFactsInAStatusLine() {
        Game game = game();
        TextView overlay = game.findViewById(R.id.performanceOverlay);
        com.limelight.ui.StreamMessages messages = ReflectionHelpers.getField(game, "messages");
        messages.note("This screen can’t show HDR, so the stream is in SDR.");
        setMode(game, "statistics");
        String text = overlay.getText().toString();
        assertTrue(text.startsWith("1920×1080"));
        assertTrue(text.endsWith("\nThis screen can’t show HDR, so the stream is in SDR."));
        setMode(game, "compact");
        assertFalse("The compact line stays one line", overlay.getText().toString().contains("HDR"));
    }
}

package com.limelight;

import android.app.Application;
import android.content.ClipboardManager;
import android.content.Context;
import android.os.Looper;
import android.view.View;
import android.widget.TextView;

import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HardwareMetrics;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.time.Duration;
import java.util.List;
import java.util.Map;
import java.util.function.Consumer;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameHardwareMetricsTest {
    @Test
    public void pollingRequiresVisibleForegroundStreamAndRejectsOldCallbacks() {
        Connection connection = new Connection();
        Game game = game(connection);
        update(game);
        assertEquals(1, connection.starts);
        update(game);
        assertEquals(1, connection.starts);
        Consumer<HardwareMetrics> first = connection.listener;
        deliver(first, sample(1));
        assertTrue(text(game).contains("GPU"));

        game.findViewById(R.id.performanceOverlayScroll).setVisibility(View.GONE);
        update(game);
        assertEquals(1, connection.stops);
        assertEquals("Client statistics", text(game));
        game.findViewById(R.id.performanceOverlayScroll).setVisibility(View.VISIBLE);
        update(game);
        deliver(first, sample(2));
        assertEquals("Client statistics", text(game));
        deliver(connection.listener, sample(3));
        assertTrue(text(game).contains("GPU"));

        ReflectionHelpers.setField(game, "foreground", false);
        update(game);
        assertEquals(2, connection.stops);
        assertEquals("Client statistics", text(game));
        update(game);
        assertEquals(2, connection.starts);
        ReflectionHelpers.setField(game, "foreground", true);
        ReflectionHelpers.setField(game, "isHidingOverlays", true);
        update(game);
        assertEquals(2, connection.starts);
        ReflectionHelpers.setField(game, "isHidingOverlays", false);
        ReflectionHelpers.setField(game, "connected", false);
        update(game);
        assertEquals(2, connection.starts);
    }

    @Test
    public void repeatedHostSamplesExpireAndCopiedDebugNeverContainsExpiredValues() {
        Connection connection = new Connection();
        Game game = game(connection);
        update(game);
        HardwareMetrics sample = sample(Long.MAX_VALUE - 10);
        deliver(connection.listener, sample);
        shadowOf(Looper.getMainLooper()).idleFor(Duration.ofSeconds(4));
        deliver(connection.listener, sample);
        assertTrue(text(game).contains("GPU"));
        shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMillis(1001));
        assertEquals("Client statistics", text(game));
        deliver(connection.listener, sample);
        assertEquals("Client statistics", text(game));
        ReflectionHelpers.callInstanceMethod(game.overlays, "copyPerformanceStats");
        ClipboardManager clipboard = (ClipboardManager) game.getSystemService(Context.CLIPBOARD_SERVICE);
        assertEquals("Client debug", clipboard.getPrimaryClip().getItemAt(0).getText().toString());
        ReflectionHelpers.callInstanceMethod(game.overlays, "stopHardwareMetricsPolling");
    }

    @Test
    public void missingHardwareAndFailedRequestsReplacePreviousSnapshot() {
        Connection connection = new Connection();
        Game game = game(connection);
        update(game);
        deliver(connection.listener, sample(1));
        assertTrue(text(game).contains("GPU"));
        deliver(connection.listener, new HardwareMetrics(2, 10000, Map.of(), List.of()));
        assertFalse(text(game).contains("GPU"));
        assertFalse(text(game).contains("18.0"));
        shadowOf(Looper.getMainLooper()).idleFor(Duration.ofSeconds(6));
        assertTrue(text(game).contains("CPU"));
        deliver(connection.listener, null);
        assertEquals("Client statistics", text(game));
        ReflectionHelpers.callInstanceMethod(game.overlays, "stopHardwareMetricsPolling");
    }

    @Test
    public void theCompactOverlayNeverPollsTheHostsHardware() {
        Connection connection = new Connection();
        Game game = game(connection);
        ReflectionHelpers.setField(game, "overlayMode", "compact");
        update(game);
        assertEquals(0, connection.starts);
        ReflectionHelpers.callInstanceMethod(game.overlays, "setPerformanceOverlayMode",
                ReflectionHelpers.ClassParameter.from(String.class, "statistics"));
        assertEquals(1, connection.starts);
        ReflectionHelpers.callInstanceMethod(game.overlays, "setPerformanceOverlayMode",
                ReflectionHelpers.ClassParameter.from(String.class, "compact"));
        assertEquals(1, connection.stops);
    }

    private static HardwareMetrics sample(long timestamp) {
        return new HardwareMetrics(timestamp, 1000, Map.of("cpu_usage_percent", 18.0),
                List.of(new HardwareMetrics.Gpu(0, "AMD test GPU", Map.of("gpu_usage_percent", 25.0))));
    }

    private static void deliver(Consumer<HardwareMetrics> listener, HardwareMetrics sample) {
        listener.accept(sample);
        shadowOf(Looper.getMainLooper()).idle();
    }

    private static void update(Game game) {
        ReflectionHelpers.callInstanceMethod(game.overlays, "updateHardwareMetricsPolling");
    }

    private static String text(Game game) {
        return ((TextView) game.findViewById(R.id.performanceOverlay)).getText().toString();
    }

    private static Game game(Connection connection) {
        Game game = Robolectric.buildActivity(Game.class).get();
        game.setContentView(R.layout.activity_game);
        View overlay = game.findViewById(R.id.performanceOverlayScroll);
        overlay.setVisibility(View.VISIBLE);
        ReflectionHelpers.setField(game, "performanceOverlayView", game.findViewById(R.id.performanceOverlay));
        ReflectionHelpers.setField(game, "performanceOverlayScroll", overlay);
        ReflectionHelpers.setField(game.overlays, "statisticsPerformanceText", "Client statistics");
        ReflectionHelpers.setField(game.overlays, "debugPerformanceText", "Client debug");
        // Host hardware readings belong to the Statistics and Debug overlays.
        ReflectionHelpers.setField(game, "overlayMode", "statistics");
        ReflectionHelpers.setField(game, "foreground", true);
        ReflectionHelpers.setField(game, "connected", true);
        ReflectionHelpers.setField(game, "conn", connection);
        ReflectionHelpers.callInstanceMethod(game.overlays, "updatePerformanceOverlay");
        return game;
    }

    private static class Connection extends NvConnection {
        Consumer<HardwareMetrics> listener;
        int starts;
        int stops;

        Connection() {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }

        @Override public boolean startHardwareMetricsPolling(Consumer<HardwareMetrics> listener) {
            starts++;
            this.listener = listener;
            return true;
        }

        @Override public void stopHardwareMetricsPolling() { stops++; }
    }
}

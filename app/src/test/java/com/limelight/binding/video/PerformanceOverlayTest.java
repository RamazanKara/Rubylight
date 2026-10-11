package com.limelight.binding.video;

import android.app.Application;
import android.content.Context;
import android.content.res.Configuration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.util.Locale;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class PerformanceOverlayTest {
    private Context context(Locale locale) {
        Context app = RuntimeEnvironment.getApplication();
        Configuration config = new Configuration(app.getResources().getConfiguration());
        config.setLocale(locale);
        return app.createConfigurationContext(config);
    }

    @Test
    public void statisticsSeparatesMeasuredStagesInReadableLines() {
        String text = PerformanceOverlay.statisticsText(context(Locale.US), "1920×1080 · AV1 · HDR",
                59.94f, 60, 20.4f, 0.14f, 8, 1, 2.1f, 4.6f, 1.2f).toString();
        assertEquals("1920×1080 · AV1 · HDR\n" +
                "Displayed: 59.9 FPS · Received: 60.0 FPS\n" +
                "Video payload: 20.4 Mbps · Loss: 0.14%\n" +
                "Network RTT: 8 ms (±1 ms)\n" +
                "Host processing: 2.10 ms\n" +
                "Decode: 4.60 ms · Display wait: 1.20 ms", text);
        assertFalse(text.contains("13 ms"));
    }

    @Test
    public void unavailableReadingsAreNotDisplayedAsZerosOrGuessedFromOtherStages() {
        for (float missing : new float[] {-1, Float.NaN, Float.NEGATIVE_INFINITY, Float.POSITIVE_INFINITY}) {
            String text = PerformanceOverlay.statisticsText(context(Locale.US), "1280×720 · PyroWave · SDR",
                    missing, 60, missing, missing, missing, missing, missing, missing, missing).toString();
            assertTrue(text.contains("Displayed: — FPS · Received: 60.0 FPS"));
            assertTrue(text.contains("Video payload: — Mbps · Loss: —%"));
            assertTrue(text.contains("Network RTT: — ms (±— ms)"));
            assertTrue(text.contains("Host processing: — ms"));
            assertTrue(text.contains("Decode: — ms · Display wait: — ms"));
            assertFalse(text.contains("0.00"));
        }
        assertEquals("0.00", PerformanceOverlay.measurement(Locale.US, 0, "%.2f"));
    }

    @Test
    public void germanUsesLocalizedLabelsAndMeasurements() {
        String text = PerformanceOverlay.statisticsText(context(Locale.GERMANY), "1920×1080 · HEVC · SDR",
                59.94f, 60, 20.4f, 0.14f, 8, 1, 2.1f, 4.6f, 1.2f).toString();
        assertTrue(text.contains("Angezeigt: 59,9 FPS"));
        assertTrue(text.contains("Videodaten: 20,4 Mbit/s · Verlust: 0,14%"));
        assertTrue(text.contains("Host-Verarbeitung: 2,10 ms"));
    }

    @Test
    public void displayedFpsTracksPresentationInsteadOfReceivedFrames() {
        FrameLatencyStats stats = new FrameLatencyStats();
        long startNs = 1000000000L;
        stats.onFrameReceived(startNs);
        for (int i = 1; i <= 120; i++) {
            long receiveNs = startNs + i * 16666666L;
            stats.onFrameReceived(receiveNs);
            stats.onDecoderInput(i, i, receiveNs, receiveNs, receiveNs, (char) 21);
            stats.onDecoderOutput(0, i, receiveNs + 4000000L);
            stats.onOutputReleased(0, receiveNs + 5000000L, true, true);
            if (i % 2 == 0) stats.onFrameRendered(i, receiveNs + 8000000L);
        }
        float[] rates = stats.getFrameRates(startNs + 2020000000L);
        double[][] summary = stats.summarize();
        String text = PerformanceOverlay.statisticsText(context(Locale.US), "1280×720 · PyroWave · SDR",
                rates[2], rates[0], 12, 1.2f, 8, 1, PerformanceOverlay.average(summary, 4),
                stats.takeDecodeTimeMs(), PerformanceOverlay.average(summary, 2)).toString();
        assertTrue(text.contains("Displayed: 30.0 FPS"));
        assertTrue(text.contains("Host processing: 2.10 ms"));
        assertTrue(text.contains("Decode: 4.00 ms · Display wait: 4.00 ms"));
    }

    @Test
    public void missingDisplayTimingAndHostSamplesRemainUnavailable() {
        for (boolean pyroWave : new boolean[] {false, true}) {
            FrameLatencyStats stats = new FrameLatencyStats();
            stats.onFrameReceived(1000000000L);
            stats.onDecoderInput(1, 1, 1000000000L, 1000000000L, 1000000000L, (char) 0);
            stats.onDecoderOutput(0, 1, 1004000000L);
            stats.onOutputReleased(0, 1005000000L, true, pyroWave);
            if (pyroWave) stats.discard(1, "render_unavailable");
            double[][] summary = stats.summarize();
            assertEquals(-1, stats.getFrameRates(2000000000L)[2], 0);
            assertEquals(-1, PerformanceOverlay.average(summary, 2), 0);
            assertEquals(-1, PerformanceOverlay.average(summary, 4), 0);
            assertEquals(4, stats.takeDecodeTimeMs(), 0);
        }
    }

    @Test
    public void bitrateUsesActualElapsedTimeAndZeroTrafficIsAnObservedZero() {
        assertEquals(20, PerformanceOverlay.videoMbps(2500000, 1000000000L), 0);
        assertEquals(10, PerformanceOverlay.videoMbps(2500000, 2000000000L), 0);
        assertEquals(0, PerformanceOverlay.videoMbps(0, 1000000000L), 0);
        assertEquals(-1, PerformanceOverlay.videoMbps(2500000, 0), 0);
        assertEquals(24000, PerformanceOverlay.videoMbps(3000000000L, 1000000000L), 0);
    }

    @Test
    public void modeMigrationPreservesDetailedChoiceAndDefaultsToStatistics() {
        for (String value : new String[] {null, "compact", "statistics", "unknown"}) {
            assertEquals("statistics", PerformanceOverlay.normalizeMode(value));
        }
        assertEquals("debug", PerformanceOverlay.normalizeMode("advanced"));
        assertEquals("debug", PerformanceOverlay.normalizeMode("debug"));
    }

    @Test
    public void debugRetainsTimingPercentilesAndDriverFallbackDetails() {
        String detail = "GPU: Turnip\nFallback: system driver\n" +
                "Delay in ms: avg/p95/p99\nDecode: 4.00 / 5.00 / 6.00 (n=600)";
        String text = PerformanceOverlay.debugText(context(Locale.US), "1280×720 · PyroWave · HDR",
                "Network RTT: — ms", detail).toString();
        assertTrue(text.startsWith("Video\n1280×720"));
        assertTrue(text.contains("\n\nNetwork\nNetwork RTT: — ms"));
        assertTrue(text.endsWith("\n\nDecode\n" + detail));
    }
}

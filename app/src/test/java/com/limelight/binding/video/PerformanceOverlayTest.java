package com.limelight.binding.video;

import android.app.Application;
import android.content.Context;
import android.content.res.Configuration;

import com.limelight.nvstream.av.video.HostTimingReport;
import com.limelight.nvstream.http.HardwareMetrics;

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
                59.94f, 60, 20.4f, 0.14f, 8, 1, 2.1f, -1, 4.6f, 1.2f, false).toString();
        assertEquals("1920×1080 · AV1 · HDR\n" +
                "Rendered: 59.9 FPS · Received: 60.0 FPS\n" +
                "Video payload: 20.4 Mbps · Loss: 0.14%\n" +
                "Network RTT: 8 ms (±1 ms)\n" +
                "Host processing: 2.1 ms\n" +
                "Decode: 4.60 ms · Display wait: 1.20 ms", text);
        assertFalse(text.contains("13 ms"));
    }

    @Test
    public void pyrowaveSubmissionIsLabelledAndNeverMixedWithCompletionPercentiles() {
        Context context = context(Locale.US);
        String statistics = PerformanceOverlay.statisticsText(context, "1920×1080 · PyroWave · SDR",
                120, 120, 900, 0, 4, 1, 1.5f, -1, 0.76f, 9.4f, true).toString();
        assertTrue(statistics.contains("CPU submission: 0.76 ms · Display wait: 9.40 ms"));

        double[][] completed = new double[5][4];
        completed[1] = new double[] {600, 6.77, 8.83, 10.65};
        String debug = MediaCodecDecoderRenderer.formatLatencyOverlay(context, completed, 0.76f, true);
        assertTrue(debug.contains("Completed decode: 6.77 / 8.83 / 10.65 (n=600)"));
        assertFalse(debug.contains("0.76 / 8.83"));
    }

    @Test
    public void conventionalTimingRowKeepsItsExistingAverage() {
        double[][] completed = new double[5][4];
        completed[1] = new double[] {600, 4.6, 7, 8};
        String debug = MediaCodecDecoderRenderer.formatLatencyOverlay(context(Locale.US), completed, 3.5f);
        assertTrue(debug.contains("Decode time: 3.50 / 7.00 / 8.00 (n=600)"));
        assertFalse(debug.contains("Completed decode"));
    }

    @Test
    public void unavailableReadingsAreNotDisplayedAsZerosOrGuessedFromOtherStages() {
        for (float missing : new float[] {-1, Float.NaN, Float.NEGATIVE_INFINITY, Float.POSITIVE_INFINITY}) {
            String text = PerformanceOverlay.statisticsText(context(Locale.US), "1280×720 · PyroWave · SDR",
                    missing, 60, missing, missing, missing, missing, missing, missing, missing, missing, false).toString();
            assertTrue(text.contains("Rendered: — FPS · Received: 60.0 FPS"));
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
                59.94f, 60, 20.4f, 0.14f, 8, 1, 2.1f, -1, 4.6f, 1.2f, false).toString();
        assertTrue(text.contains("Gerendert: 59,9 FPS"));
        assertTrue(text.contains("Videodaten: 20,4 Mbit/s · Verlust: 0,14%"));
        assertTrue(text.contains("Host-Verarbeitung: 2,1 ms"));
    }

    @Test
    public void renderedFpsTracksSubmittedFramesInsteadOfReceivedOrConfirmedFrames() {
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
                PerformanceOverlay.renderedFps(rates), rates[0], 12, 1.2f, 8, 1, PerformanceOverlay.average(summary, 4),
                PerformanceOverlay.average(summary, 7), stats.takeDecodeTimeMs(),
                PerformanceOverlay.average(summary, 2), false).toString();
        // Every frame is released for rendering; only half get a render callback (rates[2] is 30).
        assertEquals(30, rates[2], 0.5f);
        assertTrue(text.contains("Rendered: 60.0 FPS"));
        assertTrue(text.contains("Host processing: 2.1 ms"));
        assertTrue(text.contains("Decode: 4.00 ms · Display wait: 4.00 ms"));
    }

    @Test
    public void compactDotJudgesCompositorConfirmedFramesWhenReported() {
        float[] confirmed = {60, 60, 30};
        float[] unreported = {60, 60, -1};
        assertEquals(60, PerformanceOverlay.renderedFps(confirmed), 0);
        assertEquals(30, PerformanceOverlay.healthFps(confirmed), 0);
        assertEquals(60, PerformanceOverlay.healthFps(unreported), 0);
        // The number shown is the rendered rate; on-screen drops still turn the dot red.
        CharSequence poor = PerformanceOverlay.compactText(context(Locale.US),
                PerformanceOverlay.renderedFps(confirmed), PerformanceOverlay.healthFps(confirmed), 60, 8, 0, 4);
        assertTrue(poor.toString().contains("60.0 FPS"));
        assertEquals(PerformanceOverlay.Health.POOR, healthOf(poor));
        CharSequence good = PerformanceOverlay.compactText(context(Locale.US),
                PerformanceOverlay.renderedFps(unreported), PerformanceOverlay.healthFps(unreported), 60, 8, 0, 4);
        assertEquals(PerformanceOverlay.Health.GOOD, healthOf(good));
    }

    private static PerformanceOverlay.Health healthOf(CharSequence compact) {
        PerformanceOverlay.HealthSpan[] spans = ((android.text.Spanned) compact)
                .getSpans(0, 1, PerformanceOverlay.HealthSpan.class);
        assertEquals(1, spans.length);
        return spans[0].health;
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

    static HostTimingReport hostReport(long hostUs, long queueUs, long encodeUs, long sendUs) {
        long[] fields = {1, hostUs, queueUs, encodeUs, sendUs};
        byte[] data = new byte[4 + 20];
        data[0] = 1;
        data[1] = 1;
        for (int field = 0; field < 5; field++) {
            for (int b = 0; b < 4; b++) data[4 + 4 * field + b] = (byte) (fields[field] >> (8 * b));
        }
        return HostTimingReport.parse(data);
    }

    // Regression: the overlay's "Host processing" once showed the host timing reports' present to
    // last packet sent (4.92 ms on a REDMAGIC tablet) where Artemis showed the header's 0.5 ms.
    @Test
    public void aHostTimingReportNeverChangesTheHostProcessingLine() {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onDecoderInput(1, 1, 1000000000L, 1000000000L, 1000000000L, (char) 5);
        stats.onHostTiming(hostReport(4900, 100, 600, 4200), 1000000000L);
        double[][] summary = stats.summarize();
        float header = PerformanceOverlay.average(summary, 4);
        String without = PerformanceOverlay.statisticsText(context(Locale.US), "1280×720 · PyroWave · SDR",
                120, 120, 450, 0, 8, 1, header, -1, 1, 1, false).toString();
        String with = PerformanceOverlay.statisticsText(context(Locale.US), "1280×720 · PyroWave · SDR",
                120, 120, 450, 0, 8, 1, header, PerformanceOverlay.average(summary, FrameLatencyStats.HOST_SENT_STAGE),
                1, 1, false).toString();
        assertTrue(with, with.contains("Host processing: 0.5 ms\n"));
        assertTrue(with, with.contains("Host to network: 4.9 ms\n"));
        assertFalse(with, with.contains("send"));
        // Same Host processing line with and without a report; only the extra line differs.
        assertTrue(without, without.contains("Host processing: 0.5 ms\n"));
        assertFalse(without, without.contains("Host to network"));
        assertEquals(without.replace("Host processing: 0.5 ms\n", "Host processing: 0.5 ms\nHost to network: 4.9 ms\n"), with);
        String german = PerformanceOverlay.statisticsText(context(Locale.GERMANY), "1280×720 · PyroWave · SDR",
                120, 120, 450, 0, 8, 1, 0.5f, 4.9f, 1, 1, false).toString();
        assertTrue(german, german.contains("Host-Verarbeitung: 0,5 ms\nHost bis Netzwerk: 4,9 ms"));
    }

    @Test
    public void hostProcessingIsTheHeaderAverageLikeArtemisComputesIt() {
        // Artemis: total of the frames' 100 us values over the frames that carried one, divided by 10.
        VideoStats window = new VideoStats();
        window.totalHostProcessingLatency = 5 + 6 + 4;
        window.framesWithHostProcessingLatency = 3;
        assertEquals(0.5f, window.getHostProcessingMs(), 0.00001f);
        VideoStats lastTwo = new VideoStats();
        lastTwo.add(window);
        VideoStats next = new VideoStats();
        next.totalHostProcessingLatency = 30;
        next.framesWithHostProcessingLatency = 3;
        lastTwo.add(next);
        assertEquals(0.75f, lastTwo.getHostProcessingMs(), 0.00001f);
        assertEquals(-1, new VideoStats().getHostProcessingMs(), 0);
    }

    @Test
    public void theDebugRowsKeepHostProcessingAndPutThePresentToSentUnderItsOwnLabel() {
        double[][] summary = new double[9][4];
        summary[4] = new double[] {600, 0.5, 1, 1.5};
        String header = MediaCodecDecoderRenderer.formatLatencyOverlay(context(Locale.US), summary, -1);
        assertTrue(header, header.contains("Host processing: 0.50 / 1.00 / 1.50 (n=600)"));
        assertFalse(header, header.contains("+ send"));
        summary[FrameLatencyStats.HOST_SENT_STAGE] = new double[] {600, 5, 6, 7};
        String reports = MediaCodecDecoderRenderer.formatLatencyOverlay(context(Locale.US), summary, -1);
        assertTrue(reports, reports.contains("Host processing: 0.50 / 1.00 / 1.50 (n=600)"));
        assertTrue(reports, reports.contains("Host to network: 5.00 / 6.00 / 7.00 (n=600)"));
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
    public void modeNamesKeepDetailedChoicesAndDefaultToCompact() {
        for (String value : new String[] {null, "compact", "unknown", ""}) {
            assertEquals("compact", PerformanceOverlay.normalizeMode(value));
        }
        assertEquals("statistics", PerformanceOverlay.normalizeMode("statistics"));
        assertEquals("statistics", PerformanceOverlay.normalizeMode("expanded"));
        assertEquals("debug", PerformanceOverlay.normalizeMode("advanced"));
        assertEquals("debug", PerformanceOverlay.normalizeMode("debug"));
    }

    @Test
    public void compactIsOneLineOfShownFpsRoundTripAndLoss() {
        CharSequence text = PerformanceOverlay.compactText(context(Locale.US), 119.94f, 120, 8, 0.14f, 4.6f);
        assertEquals("● 119.9 FPS · 8 ms RTT · 0.1% loss", text.toString());
        assertFalse(text.toString().contains("\n"));
        // The figure is the measured network round trip, labelled as one: never a sum passed off as latency.
        assertFalse(text.toString().contains("13 ms"));
        assertEquals(0xff5fd38d, dotColor(text));
        assertEquals("Stream good: 119.9 FPS · 8 ms RTT · 0.1% loss",
                PerformanceOverlay.describeCompact(context(Locale.US), text));
    }

    @Test
    public void aLoweredBitrateIsAmberLastLineInEveryModeAndNothingWithoutANotice() {
        Context context = context(Locale.US);
        LinkNoticeStatus status = LinkNoticeStatus.parse(LinkNoticeStatus.STATE_LIMITED, 150_000, 37_500);
        CharSequence compact = PerformanceOverlay.compactText(context, 119.94f, 120, 8, 0.14f, 4.6f);
        CharSequence with = PerformanceOverlay.withLinkNotice(context, compact, status);
        assertEquals("● 119.9 FPS · 8 ms RTT · 0.1% loss\nHost lowered the bitrate to 38 Mbps: network", with.toString());
        assertEquals(0xff5fd38d, dotColor(with));
        android.text.Spanned spanned = (android.text.Spanned) with;
        android.text.style.ForegroundColorSpan[] amber = spanned.getSpans(compact.length(), with.length(),
                android.text.style.ForegroundColorSpan.class);
        assertEquals(1, amber.length);
        assertEquals(0xfff5d06b, amber[0].getForegroundColor());
        assertEquals(compact.length(), spanned.getSpanStart(amber[0]));
        assertSame(compact, PerformanceOverlay.withLinkNotice(context, compact, null));
        String statistics = PerformanceOverlay.statisticsText(context, "AV1", 60, 60, 20, 0, 8, 1, 2, -1, 4, 1, false).toString();
        assertTrue(PerformanceOverlay.withLinkNotice(context, statistics, status).toString()
                .endsWith("Decode: 4.00 ms · Display wait: 1.00 ms\nHost lowered the bitrate to 38 Mbps: network"));
    }

    @Test
    public void theNoticeSaysMbpsInTheUsersLocaleAndTheCompactDescriptionCarriesIt() {
        LinkNoticeStatus floor = LinkNoticeStatus.parse(LinkNoticeStatus.STATE_FLOOR, 150_000, 4_500);
        assertEquals("Host lowered the bitrate to 4.5 Mbps: network",
                PerformanceOverlay.linkNoticeText(context(Locale.US), floor));
        assertEquals("Host hat die Bitrate auf 4,5 Mbit/s gesenkt: Netzwerk",
                PerformanceOverlay.linkNoticeText(context(Locale.GERMANY), floor));
        CharSequence compact = PerformanceOverlay.compactText(context(Locale.US), 119.94f, 120, 8, 0.14f, 4.6f);
        assertEquals("Stream good: 119.9 FPS · 8 ms RTT · 0.1% loss. Host lowered the bitrate to 4.5 Mbps: network",
                PerformanceOverlay.describeCompact(context(Locale.US), compact, floor));
        assertEquals(PerformanceOverlay.describeCompact(context(Locale.US), compact),
                PerformanceOverlay.describeCompact(context(Locale.US), compact, null));
        assertNull(PerformanceOverlay.describeCompact(context(Locale.US), "", floor));
    }

    @Test
    public void compactLeavesOutUnmeasuredValuesAndWaitsWithoutAnyReading() {
        for (float missing : new float[] {-1, Float.NaN, Float.POSITIVE_INFINITY}) {
            assertEquals("● 60.0 FPS · 0.0% loss",
                    PerformanceOverlay.compactText(context(Locale.US), 60, 60, missing, 0, 4).toString());
            assertEquals("● 12 ms RTT",
                    PerformanceOverlay.compactText(context(Locale.US), missing, missing, 12, missing, missing).toString());
            assertEquals("", PerformanceOverlay.compactText(context(Locale.US), missing, 60, missing, missing, 4).toString());
        }
        assertNull(PerformanceOverlay.describeCompact(context(Locale.US), ""));
    }

    @Test
    public void compactDotShowsHealthFromLossRoundTripAndShownFrames() {
        assertEquals(PerformanceOverlay.Health.GOOD, PerformanceOverlay.health(119.9f, 120, 8, 0.1f, 4));
        assertEquals(PerformanceOverlay.Health.FAIR, PerformanceOverlay.health(119.9f, 120, 8, 1.2f, 4));
        assertEquals(PerformanceOverlay.Health.FAIR, PerformanceOverlay.health(119.9f, 120, 45, 0, 4));
        assertEquals(PerformanceOverlay.Health.FAIR, PerformanceOverlay.health(100, 120, 8, 0, 4));
        // Decoding slower than one frame of the received rate cannot keep up.
        assertEquals(PerformanceOverlay.Health.FAIR, PerformanceOverlay.health(119.9f, 120, 8, 0, 9));
        assertEquals(PerformanceOverlay.Health.POOR, PerformanceOverlay.health(119.9f, 120, 8, 6, 4));
        assertEquals(PerformanceOverlay.Health.POOR, PerformanceOverlay.health(119.9f, 120, 90, 0, 4));
        assertEquals(PerformanceOverlay.Health.POOR, PerformanceOverlay.health(80, 120, 8, 0, 4));
        assertEquals(PerformanceOverlay.Health.UNKNOWN, PerformanceOverlay.health(-1, 60, -1, -1, 4));
        CharSequence poor = PerformanceOverlay.compactText(context(Locale.US), 30, 60, 120, 7, 4);
        assertEquals(0xffffb4ab, dotColor(poor));
        assertTrue(PerformanceOverlay.describeCompact(context(Locale.US), poor).startsWith("Stream poor: "));
        // Shape says it as well as colour: a triangle for fair, a cross for poor.
        assertTrue(poor.toString().startsWith("✕ "));
        CharSequence fair = PerformanceOverlay.compactText(context(Locale.US), 119.9f, 120, 8, 1.2f, 4);
        assertTrue(fair.toString().startsWith("▲ "));
        assertTrue(PerformanceOverlay.describeCompact(context(Locale.US), fair).startsWith("Stream struggling: "));
    }

    @Test
    public void overlayColoursAreTheNightStatusTokens() {
        // Over video the overlay uses the media (night) tones whatever the app theme is.
        Context context = context(Locale.US);
        assertEquals(androidx.core.content.ContextCompat.getColor(context, com.limelight.R.color.night_status_ok),
                PerformanceOverlay.healthColor(PerformanceOverlay.Health.GOOD));
        assertEquals(androidx.core.content.ContextCompat.getColor(context, com.limelight.R.color.night_status_warn),
                PerformanceOverlay.healthColor(PerformanceOverlay.Health.FAIR));
        assertEquals(androidx.core.content.ContextCompat.getColor(context, com.limelight.R.color.night_error),
                PerformanceOverlay.healthColor(PerformanceOverlay.Health.POOR));
        assertEquals(androidx.core.content.ContextCompat.getColor(context, com.limelight.R.color.night_on_surface_variant),
                PerformanceOverlay.healthColor(PerformanceOverlay.Health.UNKNOWN));
        assertEquals(androidx.core.content.ContextCompat.getColor(context, com.limelight.R.color.night_tertiary),
                PerformanceOverlay.MEDIA_INFO);
    }

    @Test
    public void germanCompactUsesLocalizedNumbersAndWords() {
        CharSequence text = PerformanceOverlay.compactText(context(Locale.GERMANY), 59.94f, 60, 8, 0.14f, 4.6f);
        assertEquals("● 59,9 FPS · 8 ms RTT · 0,1% Verlust", text.toString());
        assertTrue(PerformanceOverlay.describeCompact(context(Locale.GERMANY), text).startsWith("Stream läuft gut: "));
    }

    private static int dotColor(CharSequence text) {
        android.text.style.ForegroundColorSpan[] spans = ((android.text.Spanned) text).getSpans(0, 1,
                android.text.style.ForegroundColorSpan.class);
        assertEquals(1, spans.length);
        return spans[0].getForegroundColor();
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

    @Test
    public void hardwareStatisticsIdentifyEachHostGpuAndPreserveMissingValues() throws Exception {
        HardwareMetrics metrics = HardwareMetrics.parse("{\"version\":1,\"sampled_at_unix_ms\":1000,\"interval_ms\":1000," +
                "\"cpu\":{\"cpu_usage_percent\":41.6},\"gpus\":[" +
                "{\"id\":11520,\"name\":\"AMD Radeon\",\"metrics\":{\"gpu_usage_percent\":50,\"gpu_temperature_c\":47,\"gpu_power_w\":28}}," +
                "{\"id\":7,\"name\":\"\",\"metrics\":{\"gpu_usage_percent\":0}}]}");
        String text = PerformanceOverlay.withHardwareMetrics(context(Locale.US), "Stream stats", metrics, false).toString();
        assertEquals("Stream stats\nHost CPU: 41.6%\nHost GPU 11520: 50.0% · 47.0 °C · 28.0 W\n" +
                "Host GPU 7: 0.0% · — °C · — W", text);
        assertFalse(text.contains("Encoder GPU"));
    }

    @Test
    public void hardwareDebugSeparatesGpuAndBoardPowerAndIncludesEveryReading() throws Exception {
        HardwareMetrics metrics = HardwareMetrics.parse("{\"version\":1,\"sampled_at_unix_ms\":1791712800000,\"interval_ms\":2000," +
                "\"cpu\":{},\"gpus\":[{\"id\":4,\"name\":\"AMD Radeon\",\"metrics\":{" +
                "\"gpu_usage_percent\":20,\"gpu_clock_mhz\":1200,\"gpu_memory_clock_mhz\":900," +
                "\"gpu_temperature_c\":47,\"gpu_hotspot_temperature_c\":51,\"gpu_memory_temperature_c\":50," +
                "\"gpu_voltage_mv\":950,\"gpu_fan_rpm\":0,\"gpu_fan_percent\":0," +
                "\"gpu_power_w\":28,\"gpu_board_power_w\":40,\"gpu_vram_used_mb\":2679,\"gpu_vram_usage_percent\":13.09}}]}");
        String text = PerformanceOverlay.withHardwareMetrics(context(Locale.US), "Decode\np95/p99", metrics, true).toString();
        assertTrue(text.startsWith("Decode\np95/p99\n\nHost hardware (all GPUs)\nHost CPU: —%"));
        assertTrue(text.contains("Host sample: 1791712800000 ms since Unix epoch · Interval: 2000 ms"));
        assertTrue(text.contains("GPU 4 · AMD Radeon"));
        assertTrue(text.contains("Usage: 20.0% · GPU clock: 1200 MHz · Memory clock: 900 MHz"));
        assertTrue(text.contains("GPU: 47.0 °C · Hotspot: 51.0 °C · Memory: 50.0 °C"));
        assertTrue(text.contains("Voltage: 950 mV · Fan: 0 RPM (0.0%)"));
        assertTrue(text.contains("GPU power: 28.0 W · Board power: 40.0 W"));
        assertTrue(text.contains("VRAM: 2679 MB (13.1%)"));
    }

    @Test
    public void hardwareMeasurementsLocalizeAndDoNotCarryMissingSamplesForward() throws Exception {
        HardwareMetrics metrics = HardwareMetrics.parse("{\"version\":1,\"sampled_at_unix_ms\":1000,\"interval_ms\":1000," +
                "\"cpu\":{\"cpu_usage_percent\":41.6},\"gpus\":[{\"id\":7,\"name\":\"\",\"metrics\":{}}]}");
        String text = PerformanceOverlay.withHardwareMetrics(context(Locale.GERMANY), "Stream", metrics, true).toString();
        assertTrue(text.contains("Host-CPU: 41,6%"));
        assertTrue(text.contains("GPU 7 · Unbenannte GPU"));
        assertTrue(text.contains("Lüfter: — U/min (—%)"));
        HardwareMetrics empty = HardwareMetrics.parse("{\"version\":1,\"sampled_at_unix_ms\":2000,\"interval_ms\":1000,\"cpu\":{},\"gpus\":[]}");
        assertEquals("Stream\nHost CPU: —%", PerformanceOverlay.withHardwareMetrics(context(Locale.US), "Stream", empty, false).toString());
        CharSequence base = PerformanceOverlay.statisticsText(context(Locale.US), "AV1", 60, 60, 20, 0, 8, 1, 2, -1, 4, 1, false);
        assertSame(base, PerformanceOverlay.withHardwareMetrics(context(Locale.US), base, null, false));
        assertSame(base, PerformanceOverlay.withHardwareMetrics(context(Locale.US), base, null, true));
    }
}

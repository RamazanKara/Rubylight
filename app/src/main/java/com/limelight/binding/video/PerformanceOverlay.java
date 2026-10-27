package com.limelight.binding.video;

import android.content.Context;
import android.graphics.Typeface;
import android.text.SpannableString;
import android.text.SpannableStringBuilder;
import android.text.Spanned;
import android.text.style.ForegroundColorSpan;
import android.text.style.StyleSpan;

import com.limelight.R;
import com.limelight.nvstream.http.HardwareMetrics;

import java.util.ArrayList;
import java.util.Locale;
import java.util.Map;

public final class PerformanceOverlay {
    /** One line: rendered FPS, network round trip and frame loss behind a health dot. The default. */
    public static final String MODE_COMPACT = "compact";
    /** Six lines of stream measurements. */
    public static final String MODE_STATISTICS = "statistics";
    /** Every measurement and decoder diagnostic, scrollable. */
    public static final String MODE_DEBUG = "debug";

    /** The stream's health as the compact overlay's dot shows it. */
    public enum Health { UNKNOWN, GOOD, FAIR, POOR }

    public static String normalizeMode(String value) {
        if ("debug".equals(value) || "advanced".equals(value)) return MODE_DEBUG;
        // "expanded" was the full view before Statistics and Debug existed.
        if ("statistics".equals(value) || "expanded".equals(value)) return MODE_STATISTICS;
        return MODE_COMPACT;
    }

    private static boolean available(double value) {
        return value >= 0 && Double.isFinite(value);
    }

    static String measurement(Locale locale, double value, String format) {
        return available(value) ? String.format(locale, format, value) : "—";
    }

    static float average(double[][] summary, int stage) {
        return summary[stage][0] > 0 && available(summary[stage][1]) ? (float) summary[stage][1] : -1;
    }

    static float videoMbps(long bytes, long elapsedNs) {
        return elapsedNs > 0 ? (float) (bytes * 8000.0 / elapsedNs) : -1;
    }

    /**
     * The debug overlay's frame pacing lines, as Moonlight V+'s jitter monitor shows them
     * (qiin2333/moonlight-vplus, GPL-3.0): average present interval, jitter and judder, the share
     * of frames per vsync count, and the window as a timeline. Null without a window.
     */
    @SuppressWarnings("deprecation")
    static String presentJitterText(Context context, PresentJitter.Snapshot window) {
        if (window == null || window.count == 0) return null;
        Locale locale = context.getResources().getConfiguration().locale;
        StringBuilder histogram = new StringBuilder();
        for (int units = 0; units <= PresentJitter.MAX_UNITS; units++) {
            if (window.histogram[units] == 0) continue;
            if (histogram.length() > 0) histogram.append(" · ");
            histogram.append(units).append(units == PresentJitter.MAX_UNITS ? "+" : "").append(": ")
                    .append(measurement(locale, window.percent(units), "%.1f")).append('%');
        }
        return context.getString(R.string.perf_overlay_frame_pacing,
                measurement(locale, window.averageMs, "%.1f"),
                measurement(locale, window.jitterMs, "%.2f"),
                measurement(locale, window.judderPercent, "%.1f")) + '\n' +
                context.getString(R.string.perf_overlay_vsyncs_per_frame, histogram) + '\n' +
                window.timeline();
    }

    /**
     * The "Host processing" line is the average of the video frame header's per-frame processing time,
     * the same number Artemis and Moonlight show (hostMs). It must stay that: users compare it with
     * those clients, and the host timing reports' present to last packet sent includes the network
     * send, so it reads several times higher. Do not feed a report value into hostMs, do not split
     * hostMs by the send, and do not rename the line. A report's present to sent (hostToNetworkMs,
     * -1 without reports) gets its own line below under its own label.
     */
    @SuppressWarnings("deprecation")
    public static CharSequence statisticsText(Context context, String stream, float shownFps, float receivedFps,
                                               float videoMbps, float frameLossPercent, float networkMs, float varianceMs,
                                               float hostMs, float hostToNetworkMs, float decodeMs, float displayMs,
                                               boolean submissionOnly) {
        Locale locale = context.getResources().getConfiguration().locale;
        String frames = context.getString(R.string.stats_frame_rates,
                measurement(locale, shownFps, "%.1f"), measurement(locale, receivedFps, "%.1f"));
        String throughput = context.getString(R.string.stats_throughput,
                measurement(locale, videoMbps, "%.1f"), measurement(locale, frameLossPercent, "%.2f"));
        String network = context.getString(R.string.stats_network,
                measurement(locale, networkMs, "%.0f"), measurement(locale, varianceMs, "%.0f"));
        String host = context.getString(R.string.stats_host, measurement(locale, hostMs, "%.1f"));
        if (available(hostToNetworkMs)) {
            host += "\n" + context.getString(R.string.stats_host_network, measurement(locale, hostToNetworkMs, "%.1f"));
        }
        String client = context.getString(submissionOnly ? R.string.stats_decode_submission : R.string.stats_client,
                measurement(locale, decodeMs, "%.2f"), measurement(locale, displayMs, "%.2f"));
        SpannableString text = new SpannableString(stream + '\n' + frames + '\n' + throughput + '\n' + network + '\n' + host + '\n' + client);
        text.setSpan(new StyleSpan(Typeface.BOLD), 0, stream.length(), Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
        return text;
    }

    /**
     * The dot is red for loss of 5% or more, a round trip of 80 ms or more, or a quarter of the received
     * frames not shown; amber from 1% loss, 40 ms, a tenth of the frames not shown, or decoding slower than
     * a frame; green otherwise. Grey until there is a measurement.
     */
    public static Health health(float shownFps, float receivedFps, float networkMs, float frameLossPercent,
                                float decodeMs) {
        boolean frames = available(shownFps) && available(receivedFps) && receivedFps > 0;
        if (!available(shownFps) && !available(networkMs) && !available(frameLossPercent)) return Health.UNKNOWN;
        if ((available(frameLossPercent) && frameLossPercent >= 5) || (available(networkMs) && networkMs >= 80) ||
                (frames && shownFps < receivedFps * 0.75f)) {
            return Health.POOR;
        }
        if ((available(frameLossPercent) && frameLossPercent >= 1) || (available(networkMs) && networkMs >= 40) ||
                (frames && shownFps < receivedFps * 0.9f) ||
                (available(decodeMs) && available(receivedFps) && receivedFps > 0 && decodeMs > 1000f / receivedFps)) {
            return Health.FAIR;
        }
        return Health.GOOD;
    }

    // The overlay is always over video, so it takes the night (media) tones of the shared status tokens:
    // night_status_ok, night_status_warn, night_error, night_on_surface_variant and night_tertiary in palette.xml.
    static final int MEDIA_OK = 0xff5fd38d;
    static final int MEDIA_WARN = 0xfff5d06b;
    static final int MEDIA_ERROR = 0xffffb4ab;
    static final int MEDIA_INK_2 = 0xffc1c4d2;
    static final int MEDIA_INFO = 0xffafc8e9;

    static int healthColor(Health health) {
        switch (health) {
            case GOOD: return MEDIA_OK;
            case FAIR: return MEDIA_WARN;
            case POOR: return MEDIA_ERROR;
            default: return MEDIA_INK_2;
        }
    }

    /** The compact line's mark: a dot while the stream is fine, a triangle or a cross when not, so shape says it too. */
    static String healthGlyph(Health health) {
        switch (health) {
            case FAIR: return "▲";
            case POOR: return "✕";
            default: return "●";
        }
    }

    /**
     * "● 119.9 FPS · 8 ms RTT · 0.1% loss": frames shown on this screen, the network round trip and the
     * video frames lost on the way. Each value only appears once it was measured.
     */
    public static CharSequence compactText(Context context, float shownFps, float receivedFps, float networkMs,
                                           float frameLossPercent, float decodeMs) {
        return compactText(context, shownFps, shownFps, receivedFps, networkMs, frameLossPercent, decodeMs);
    }

    /** The FPS the overlays show: frames submitted for display, which needs no render callbacks. */
    public static float renderedFps(float[] rates) {
        return rates[FrameLatencyStats.RENDERED_STAGE];
    }

    /**
     * The FPS the compact dot judges: the compositor-confirmed rate when the device reports it, so
     * frames dropped on screen still turn the dot amber or red, otherwise the rendered rate.
     */
    public static float healthFps(float[] rates) {
        float confirmed = rates[FrameLatencyStats.PRESENTED_STAGE];
        return available(confirmed) ? confirmed : renderedFps(rates);
    }

    /** As above, with the FPS the dot judges given apart from the one shown. */
    @SuppressWarnings("deprecation")
    public static CharSequence compactText(Context context, float shownFps, float healthFps, float receivedFps,
                                           float networkMs, float frameLossPercent, float decodeMs) {
        Locale locale = context.getResources().getConfiguration().locale;
        ArrayList<String> values = new ArrayList<>();
        if (available(shownFps)) {
            values.add(context.getString(R.string.overlay_compact_fps, measurement(locale, shownFps, "%.1f")));
        }
        if (available(networkMs)) {
            values.add(context.getString(R.string.overlay_compact_rtt, measurement(locale, networkMs, "%.0f")));
        }
        if (available(frameLossPercent)) {
            values.add(context.getString(R.string.overlay_compact_loss, measurement(locale, frameLossPercent, "%.1f")));
        }
        if (values.isEmpty()) return "";
        Health health = health(healthFps, receivedFps, networkMs, frameLossPercent, decodeMs);
        SpannableString text = new SpannableString(healthGlyph(health) + " " + android.text.TextUtils.join(" · ", values));
        text.setSpan(new HealthSpan(health), 0, 1, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
        return text;
    }

    /** Colours the compact overlay's dot and remembers the health it stands for. */
    static final class HealthSpan extends ForegroundColorSpan {
        final Health health;

        HealthSpan(Health health) {
            super(healthColor(health));
            this.health = health;
        }
    }

    /** The compact line with its dot in words, for screen readers that would only read "black circle". */
    public static String describeCompact(Context context, CharSequence compactText) {
        if (!(compactText instanceof Spanned) || compactText.length() < 2) return null;
        HealthSpan[] spans = ((Spanned) compactText).getSpans(0, 1, HealthSpan.class);
        Health health = spans.length == 0 ? Health.UNKNOWN : spans[0].health;
        int label = health == Health.GOOD ? R.string.overlay_health_good : health == Health.FAIR ?
                R.string.overlay_health_fair : health == Health.POOR ? R.string.overlay_health_poor :
                R.string.overlay_health_unknown;
        return context.getString(label) + ": " + compactText.subSequence(2, compactText.length());
    }

    /** The compact line's description, with the host's lowered-bitrate notice after it when there is one. */
    public static String describeCompact(Context context, CharSequence compactText, LinkNoticeStatus status) {
        String description = describeCompact(context, compactText);
        if (description == null || status == null) return description;
        return description + ". " + linkNoticeText(context, status);
    }

    public static CharSequence debugText(Context context, String video, String network, String decode) {
        SpannableStringBuilder text = new SpannableStringBuilder();
        appendSection(text, context.getString(R.string.overlay_video), video);
        appendSection(text, context.getString(R.string.overlay_network), network);
        appendSection(text, context.getString(R.string.overlay_decode), decode);
        return text;
    }

    /**
     * The status line under the statistics: lasting facts about this stream, such as HDR or a codec the
     * PC or device lacks, in a quieter colour, so the messages that said them once are not needed again.
     */
    public static CharSequence withStatusLine(CharSequence base, java.util.List<String> notes) {
        if (notes.isEmpty()) return base;
        SpannableStringBuilder text = new SpannableStringBuilder(base);
        int start = text.length();
        for (String note : notes) text.append('\n').append(note);
        text.setSpan(new ForegroundColorSpan(MEDIA_INK_2), start, text.length(), Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
        return text;
    }

    /** "Host lowered the bitrate to 9 Mbps: network", the line the overlay shows while the host holds it down. */
    public static String linkNoticeText(Context context, LinkNoticeStatus status) {
        Locale locale = context.getResources().getConfiguration().locale;
        return context.getString(R.string.link_notice_lowered, status.currentMbps(locale));
    }

    /** The overlay with the host's lowered-bitrate notice as a last line in the warn colour; the overlay itself when there is none. */
    public static CharSequence withLinkNotice(Context context, CharSequence base, LinkNoticeStatus status) {
        if (status == null) return base;
        SpannableStringBuilder text = new SpannableStringBuilder(base);
        int start = text.length();
        text.append('\n').append(linkNoticeText(context, status));
        text.setSpan(new ForegroundColorSpan(healthColor(Health.FAIR)), start, text.length(),
                Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
        return text;
    }

    @SuppressWarnings("deprecation")
    public static CharSequence withHardwareMetrics(Context context, CharSequence base,
                                                    HardwareMetrics metrics, boolean debug) {
        if (metrics == null) return base;
        Locale locale = context.getResources().getConfiguration().locale;
        StringBuilder hardware = new StringBuilder(context.getString(R.string.hardware_cpu,
                hardwareValue(locale, metrics.cpu, "cpu_usage_percent", "%.1f")));
        if (debug) {
            hardware.append('\n').append(context.getString(R.string.hardware_sample,
                    metrics.sampledAtUnixMs, metrics.intervalMs));
        }
        for (HardwareMetrics.Gpu gpu : metrics.gpus) {
            Map<String, Double> values = gpu.metrics;
            if (!debug) {
                hardware.append('\n').append(context.getString(R.string.hardware_gpu_summary, gpu.id,
                        hardwareValue(locale, values, "gpu_usage_percent", "%.1f"),
                        hardwareValue(locale, values, "gpu_temperature_c", "%.1f"),
                        hardwareValue(locale, values, "gpu_power_w", "%.1f")));
                continue;
            }
            hardware.append("\n\n").append(context.getString(R.string.hardware_gpu_name, gpu.id,
                    gpu.name.isEmpty() ? context.getString(R.string.hardware_gpu_unnamed) : gpu.name));
            hardware.append('\n').append(context.getString(R.string.hardware_gpu_clocks,
                    hardwareValue(locale, values, "gpu_usage_percent", "%.1f"),
                    hardwareValue(locale, values, "gpu_clock_mhz", "%.0f"),
                    hardwareValue(locale, values, "gpu_memory_clock_mhz", "%.0f")));
            hardware.append('\n').append(context.getString(R.string.hardware_gpu_temperatures,
                    hardwareValue(locale, values, "gpu_temperature_c", "%.1f"),
                    hardwareValue(locale, values, "gpu_hotspot_temperature_c", "%.1f"),
                    hardwareValue(locale, values, "gpu_memory_temperature_c", "%.1f")));
            hardware.append('\n').append(context.getString(R.string.hardware_gpu_fan_voltage,
                    hardwareValue(locale, values, "gpu_voltage_mv", "%.0f"),
                    hardwareValue(locale, values, "gpu_fan_rpm", "%.0f"),
                    hardwareValue(locale, values, "gpu_fan_percent", "%.1f")));
            hardware.append('\n').append(context.getString(R.string.hardware_gpu_power,
                    hardwareValue(locale, values, "gpu_power_w", "%.1f"),
                    hardwareValue(locale, values, "gpu_board_power_w", "%.1f")));
            hardware.append('\n').append(context.getString(R.string.hardware_gpu_memory,
                    hardwareValue(locale, values, "gpu_vram_used_mb", "%.0f"),
                    hardwareValue(locale, values, "gpu_vram_usage_percent", "%.1f")));
        }
        SpannableStringBuilder text = new SpannableStringBuilder(base);
        if (debug) {
            appendSection(text, context.getString(R.string.hardware_title), hardware.toString());
        } else {
            if (text.length() != 0) text.append('\n');
            text.append(hardware);
        }
        return text;
    }

    private static String hardwareValue(Locale locale, Map<String, Double> values, String key, String format) {
        Double value = values.get(key);
        return value == null ? "—" : String.format(locale, format, value);
    }

    private static void appendSection(SpannableStringBuilder text, String title, String body) {
        if (text.length() != 0) text.append("\n\n");
        int start = text.length();
        text.append(title);
        text.setSpan(new StyleSpan(Typeface.BOLD), start, text.length(), Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
        text.setSpan(new ForegroundColorSpan(MEDIA_INFO), start, text.length(), Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
        text.append('\n').append(body);
    }
}

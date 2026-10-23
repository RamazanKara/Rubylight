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
    /** One line: displayed FPS, network round trip and frame loss behind a health dot. The default. */
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
     * hostMs is the host's present to last packet sent. When the host's timing reports give the send
     * stage (sendMs, first to last packet) the host line shows the two apart, so the network send is
     * not read as processing; with sendMs unavailable (-1) the line keeps the single value.
     */
    @SuppressWarnings("deprecation")
    public static CharSequence statisticsText(Context context, String stream, float shownFps, float receivedFps,
                                               float videoMbps, float frameLossPercent, float networkMs, float varianceMs,
                                               float hostMs, float sendMs, float decodeMs, float displayMs,
                                               boolean submissionOnly) {
        Locale locale = context.getResources().getConfiguration().locale;
        String frames = context.getString(R.string.stats_frame_rates,
                measurement(locale, shownFps, "%.1f"), measurement(locale, receivedFps, "%.1f"));
        String throughput = context.getString(R.string.stats_throughput,
                measurement(locale, videoMbps, "%.1f"), measurement(locale, frameLossPercent, "%.2f"));
        String network = context.getString(R.string.stats_network,
                measurement(locale, networkMs, "%.0f"), measurement(locale, varianceMs, "%.0f"));
        String host = available(hostMs) && available(sendMs) ?
                context.getString(R.string.stats_host_split,
                        measurement(locale, Math.max(0, hostMs - sendMs), "%.2f"), measurement(locale, sendMs, "%.2f")) :
                context.getString(R.string.stats_host, measurement(locale, hostMs, "%.2f"));
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

    static int healthColor(Health health) {
        switch (health) {
            case GOOD: return 0xff69db7c;
            case FAIR: return 0xffffc857;
            case POOR: return 0xffff6b6b;
            default: return 0xff9aa0ae;
        }
    }

    /**
     * "● 119.9 FPS · 8 ms RTT · 0.1% loss": frames shown on this screen, the network round trip and the
     * video frames lost on the way. Each value only appears once it was measured.
     */
    @SuppressWarnings("deprecation")
    public static CharSequence compactText(Context context, float shownFps, float receivedFps, float networkMs,
                                           float frameLossPercent, float decodeMs) {
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
        SpannableString text = new SpannableString("● " + android.text.TextUtils.join(" · ", values));
        text.setSpan(new HealthSpan(health(shownFps, receivedFps, networkMs, frameLossPercent, decodeMs)),
                0, 1, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
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
        text.setSpan(new ForegroundColorSpan(0xffc1c4d2), start, text.length(), Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
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
        text.setSpan(new ForegroundColorSpan(0xffa5d8ff), start, text.length(), Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
        text.append('\n').append(body);
    }
}

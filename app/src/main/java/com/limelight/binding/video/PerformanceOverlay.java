package com.limelight.binding.video;

import android.content.Context;
import android.graphics.Typeface;
import android.text.SpannableString;
import android.text.SpannableStringBuilder;
import android.text.Spanned;
import android.text.style.ForegroundColorSpan;
import android.text.style.StyleSpan;

import com.limelight.R;

import java.util.Locale;

public final class PerformanceOverlay {
    public static String normalizeMode(String value) {
        return "debug".equals(value) || "advanced".equals(value) ? "debug" : "statistics";
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

    @SuppressWarnings("deprecation")
    public static CharSequence statisticsText(Context context, String stream, float shownFps, float receivedFps,
                                               float videoMbps, float frameLossPercent, float networkMs, float varianceMs,
                                               float hostMs, float decodeMs, float displayMs) {
        Locale locale = context.getResources().getConfiguration().locale;
        String frames = context.getString(R.string.stats_frame_rates,
                measurement(locale, shownFps, "%.1f"), measurement(locale, receivedFps, "%.1f"));
        String throughput = context.getString(R.string.stats_throughput,
                measurement(locale, videoMbps, "%.1f"), measurement(locale, frameLossPercent, "%.2f"));
        String network = context.getString(R.string.stats_network,
                measurement(locale, networkMs, "%.0f"), measurement(locale, varianceMs, "%.0f"));
        String host = context.getString(R.string.stats_host, measurement(locale, hostMs, "%.2f"));
        String client = context.getString(R.string.stats_client,
                measurement(locale, decodeMs, "%.2f"), measurement(locale, displayMs, "%.2f"));
        SpannableString text = new SpannableString(stream + '\n' + frames + '\n' + throughput + '\n' + network + '\n' + host + '\n' + client);
        text.setSpan(new StyleSpan(Typeface.BOLD), 0, stream.length(), Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
        return text;
    }

    public static CharSequence debugText(Context context, String video, String network, String decode) {
        SpannableStringBuilder text = new SpannableStringBuilder();
        appendSection(text, context.getString(R.string.overlay_video), video);
        appendSection(text, context.getString(R.string.overlay_network), network);
        appendSection(text, context.getString(R.string.overlay_decode), decode);
        return text;
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

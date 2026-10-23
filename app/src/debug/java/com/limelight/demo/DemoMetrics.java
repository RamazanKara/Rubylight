package com.limelight.demo;

import android.content.Context;

import com.limelight.R;
import com.limelight.binding.video.PerfOverlayListener;
import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.binding.video.PerformanceOverlay;
import com.limelight.nvstream.http.HardwareMetrics;

import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.Locale;
import java.util.Map;

final class DemoMetrics {
    static void update(Context context, PerfOverlayListener listener, long tick) {
        float fps = 119.9f + (float) Math.sin(tick * 0.7) * 0.04f;
        float decode = 6.0f + (float) Math.sin(tick * 0.5) * 0.3f;
        String video = context.getString(R.string.perf_overlay_streamdetails, "1920x1080", 120f) + '\n' +
                context.getString(R.string.perf_overlay_incomingfps, 120f) + '\n' +
                context.getString(R.string.perf_overlay_shownfps, String.format(Locale.ROOT, "%.2f FPS", fps)) +
                "\nAV1 · 10-bit HDR\nUpscaling: FSR 1\nDemo profile · 120 Hz panel";
        String network = context.getString(R.string.perf_overlay_netdrops, 0.1f) + '\n' +
                context.getString(R.string.perf_overlay_netlatency, 8, 1) + '\n' +
                context.getString(R.string.stream_auto_bitrate_status, 40.0 + Math.sin(tick * 0.3));
        String decoder = context.getString(R.string.perf_overlay_decoder, "AV1 (scripted)") + '\n' +
                context.getString(R.string.perf_overlay_dectime, decode) + '\n' +
                context.getString(R.string.perf_overlay_low_latency, context.getString(R.string.perf_overlay_low_latency_android_vendor)) +
                "\nGPU: system driver (scripted)\nSample playback: 1080p60 SDR\n" +
                MediaCodecDecoderRenderer.formatLatencyOverlay(context, new double[][] {
                        {600, 0.4, 0.8, 1.2}, {600, decode, 7.0, 8.0}, {600, 1.2, 1.8, 2.5},
                        {600, 7.6, 9.4, 11.1}, {600, 3.1, 3.8, 4.2}
                });
        CharSequence statistics = PerformanceOverlay.statisticsText(context, "1920×1080 · AV1 · HDR",
                fps, 120, 38.6f, 0.1f, 8, 1, 3.1f, -1, decode, 1.2f, false);
        listener.onPerfUpdate(video, network, decoder,
                PerformanceOverlay.withHardwareMetrics(context, statistics, hardware(tick), false),
                PerformanceOverlay.compactText(context, fps, 120, 8, 0.1f, decode));
    }

    /** Scripted readings of the host's CPU and GPU, in the form a Rubylight host reports them. */
    static HardwareMetrics hardware(long tick) {
        Map<String, Double> cpu = new LinkedHashMap<>();
        cpu.put("cpu_usage_percent", 18.6 + Math.sin(tick * 0.6) * 1.4);
        Map<String, Double> gpu = new LinkedHashMap<>();
        gpu.put("gpu_usage_percent", 72.0 + Math.sin(tick * 0.4) * 2.5);
        gpu.put("gpu_clock_mhz", 2410.0 + Math.round(Math.sin(tick * 0.3) * 20));
        gpu.put("gpu_memory_clock_mhz", 2500.0);
        gpu.put("gpu_temperature_c", 64.0 + Math.sin(tick * 0.2) * 0.5);
        gpu.put("gpu_hotspot_temperature_c", 79.0 + Math.sin(tick * 0.2));
        gpu.put("gpu_memory_temperature_c", 74.0);
        gpu.put("gpu_voltage_mv", 1040.0);
        gpu.put("gpu_fan_rpm", 1380.0);
        gpu.put("gpu_fan_percent", 36.0);
        gpu.put("gpu_power_w", 231.0 + Math.sin(tick * 0.4) * 6);
        gpu.put("gpu_board_power_w", 252.0 + Math.sin(tick * 0.4) * 6);
        gpu.put("gpu_vram_used_mb", 10450.0);
        gpu.put("gpu_vram_usage_percent", 51.0);
        return new HardwareMetrics(1_700_000_000_000L + tick * 2000, 2000, cpu,
                Collections.singletonList(new HardwareMetrics.Gpu(0, "AMD Radeon RX 7900 XT", gpu)));
    }
}

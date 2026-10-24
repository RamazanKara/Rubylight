package com.limelight;

import android.app.Activity;
import android.os.Bundle;
import android.view.View;
import android.view.WindowManager;
import android.widget.TextView;

import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.binding.video.PerformanceOverlay;
import com.limelight.nvstream.http.HardwareMetrics;
import com.limelight.ui.StreamView;

import java.util.Collections;
import java.util.HashMap;
import java.util.Map;

/**
 * Debug-only rendering check using the stream layout and production overlay formatters and placement.
 * "--es mode compact|statistics|debug" picks the first mode (default statistics), "--ez controls true"
 * shows the on-screen controls the overlay must keep clear of. Long-press cycles the modes.
 * "--es mode health" opens the stream doctor over ten scripted minutes with a two-minute stall rhythm.
 * Debug and health show this device's real Wi-Fi reading.
 */
public class LatencyOverlaySmokeActivity extends Activity {
    private String mode;
    private com.limelight.binding.input.virtual_controller.VirtualController controls;
    private final HardwareMetrics hardwareMetrics = previewHardwareMetrics();
    private com.limelight.utils.WifiFacts wifi;
    private android.app.Dialog healthDialog;
    private static final long RHYTHM_MS = 120_000;

    private static HardwareMetrics previewHardwareMetrics() {
        Map<String, Double> gpu = new HashMap<>();
        gpu.put("gpu_usage_percent", 42.0);
        gpu.put("gpu_temperature_c", 47.0);
        gpu.put("gpu_power_w", 28.0);
        gpu.put("gpu_board_power_w", 40.0);
        gpu.put("gpu_fan_rpm", 0.0);
        gpu.put("gpu_vram_used_mb", 2679.0);
        return new HardwareMetrics(1791712800000L, 1000,
                Collections.singletonMap("cpu_usage_percent", 41.6),
                Collections.singletonList(new HardwareMetrics.Gpu(11520, "AMD Radeon (preview)", gpu)));
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_FULLSCREEN);
        getWindow().getDecorView().setSystemUiVisibility(View.SYSTEM_UI_FLAG_FULLSCREEN |
                View.SYSTEM_UI_FLAG_HIDE_NAVIGATION | View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY);
        getWindow().setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_STATE_ALWAYS_HIDDEN);
        setContentView(R.layout.activity_game);
        StreamView stream = findViewById(R.id.surfaceView);
        // This rendering fixture has no text input.
        stream.setFocusable(false);
        stream.initializeSurface(false);
        mode = savedInstanceState != null ? savedInstanceState.getString("mode") : getIntent().getStringExtra("mode");
        wifi = com.limelight.utils.WifiDiagnostics.read(this);
        // The dialog does not survive recreation; the launch intent decides, so rotation shows it again.
        if ("health".equals(getIntent().getStringExtra("mode"))) {
            showHealth();
            if ("health".equals(mode)) mode = PerformanceOverlay.MODE_DEBUG;
        }
        mode = mode == null ? PerformanceOverlay.MODE_STATISTICS : PerformanceOverlay.normalizeMode(mode);
        TextView message = findViewById(R.id.inputOnlyStatus);
        message.setText(R.string.smoke_overlay_message);
        message.setVisibility(View.VISIBLE);
        findViewById(R.id.performanceOverlayScroll).setVisibility(View.VISIBLE);
        findViewById(R.id.performanceOverlay).setOnLongClickListener(view -> {
            // Statistics, Debug, Compact, then Statistics again.
            mode = PerformanceOverlay.MODE_STATISTICS.equals(mode) ? PerformanceOverlay.MODE_DEBUG :
                    PerformanceOverlay.MODE_DEBUG.equals(mode) ? PerformanceOverlay.MODE_COMPACT : PerformanceOverlay.MODE_STATISTICS;
            findViewById(R.id.performanceOverlayScroll).scrollTo(0, 0);
            updateOverlay();
            return true;
        });
        android.widget.FrameLayout container = (android.widget.FrameLayout) stream.getParent();
        if (getIntent().getBooleanExtra("controls", false)) {
            controls = new com.limelight.binding.input.virtual_controller.VirtualController(null, container, this);
            controls.refreshLayout();
            controls.show();
        }
        com.limelight.ui.OverlayPlacement.attach(findViewById(R.id.performanceOverlayScroll), container, out -> {
            if (controls != null) controls.collectVisibleBounds(out);
        });
        updateOverlay();
    }

    private com.limelight.utils.WifiDiagnostics.Lock lock() {
        // The fixture streams nothing, so it holds no Wi-Fi lock.
        return android.os.Build.VERSION.SDK_INT < android.os.Build.VERSION_CODES.Q ?
                com.limelight.utils.WifiDiagnostics.Lock.UNSUPPORTED : com.limelight.utils.WifiDiagnostics.Lock.NOT_HELD;
    }

    // Ten minutes of 60 FPS samples with a three-second loss burst every two minutes; the last 30 s are clean.
    private void showHealth() {
        com.limelight.utils.StreamHealth health = new com.limelight.utils.StreamHealth();
        for (int second = 1; second <= 600; second++) {
            boolean stall = second % 120 >= 30 && second % 120 < 33;
            health.add(new com.limelight.utils.StreamHealth.Sample(second * 1000L, true, 60, stall ? 6 : 0.1f, 2,
                    4.6f, 2.1f, 1.2f, 60, 60, 60, 0, false));
        }
        com.limelight.utils.StreamHealth.Report report = health.report(600_000);
        healthDialog = new com.google.android.material.dialog.MaterialAlertDialogBuilder(this).setTitle(R.string.health_title)
                .setMessage(Game.healthText(this, report, com.limelight.utils.WifiDiagnostics.describe(this, wifi, lock(), 0)))
                .setPositiveButton(android.R.string.ok, null).show();
    }

    @Override
    protected void onDestroy() {
        if (healthDialog != null) healthDialog.dismiss();
        super.onDestroy();
    }

    @Override
    protected void onSaveInstanceState(Bundle outState) {
        outState.putString("mode", mode);
        super.onSaveInstanceState(outState);
    }

    private void updateOverlay() {
        TextView overlay = findViewById(R.id.performanceOverlay);
        boolean debug = PerformanceOverlay.MODE_DEBUG.equals(mode);
        overlay.setTextSize(PerformanceOverlay.MODE_COMPACT.equals(mode) ? 13 : debug ? 11 : 12);
        overlay.setContentDescription(null);
        if (PerformanceOverlay.MODE_COMPACT.equals(mode)) {
            CharSequence compact = PerformanceOverlay.compactText(this, 119.94f, 120, 8, 0.14f, 4.6f);
            overlay.setText(compact);
            overlay.setContentDescription(PerformanceOverlay.describeCompact(this, compact));
            return;
        }
        if (!debug) {
            overlay.setText(PerformanceOverlay.withHardwareMetrics(this,
                    PerformanceOverlay.statisticsText(this, "1280×720 · AV1 · HDR",
                            119.94f, 120, 14.2f, 0.14f, 8, 1, 2.1f, -1, 4.6f, 1.2f, false), hardwareMetrics, false));
            return;
        }
        String video = getString(R.string.perf_overlay_streamdetails, "1280x720", 120f) + '\n' +
                getString(R.string.perf_overlay_incomingfps, 120f) + '\n' +
                getString(R.string.perf_overlay_onscreenfps, "119.94 FPS") + '\n' +
                getString(R.string.perf_overlay_shownfps, "119.94 FPS") + '\n' +
                getString(R.string.perf_overlay_presentdrops, 0) + "\nYUV 4:2:0 10-bit\n" +
                "Panel: 120.00 Hz · render: 120.00 Hz · VRR: max Hz";
        String network = getString(R.string.perf_overlay_netdrops, 0.14f) + '\n' +
                getString(R.string.perf_overlay_netlatency, 8, 1) + '\n' +
                getString(R.string.stream_auto_bitrate_status, 15.0) + '\n' +
                com.limelight.utils.WifiDiagnostics.describe(this, wifi, lock(), RHYTHM_MS);
        String decode = getString(R.string.perf_overlay_decoder, "c2.qti.av1.decoder (video/av01)") + '\n' +
                getString(R.string.perf_overlay_low_latency, getString(R.string.perf_overlay_low_latency_android_vendor)) + '\n' +
                getString(R.string.perf_overlay_low_latency_keys, "low-latency=1") + '\n' +
                getString(R.string.perf_overlay_low_latency_unconfirmed, "none") + '\n' +
                getString(R.string.perf_overlay_phone_hints, getString(R.string.stream_disabled)) + '\n' +
                getString(R.string.perf_overlay_dectime, 4.6f) + '\n' +
                MediaCodecDecoderRenderer.formatLatencyOverlay(this, new double[][] {
                        {600, 0.4, 0.8, 1.2}, {600, 4.6, 5.8, 6.4}, {600, 1.2, 1.8, 2.5},
                        {600, 6.4, 8.4, 10.1}, {600, 2.1, 2.8, 3.2}
                });
        overlay.setText(PerformanceOverlay.withHardwareMetrics(this,
                PerformanceOverlay.debugText(this, video, network, decode), hardwareMetrics, true));
    }
}

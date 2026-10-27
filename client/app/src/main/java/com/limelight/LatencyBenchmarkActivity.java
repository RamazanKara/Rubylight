package com.limelight;

import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Intent;
import android.media.AudioManager;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.view.Choreographer;
import android.view.Display;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.widget.Button;
import android.widget.FrameLayout;
import android.widget.TextView;
import android.widget.Toast;

import com.limelight.ui.NightjarActivity;

import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.binding.video.DecoderBenchmark;
import com.limelight.preferences.GlPreferences;
import com.limelight.utils.DeviceResults;
import com.limelight.utils.HelpLauncher;
import com.limelight.utils.LatencyBenchmarkStats;
import com.limelight.ui.TopAppBar;
import com.limelight.utils.UiHelper;

public class LatencyBenchmarkActivity extends NightjarActivity implements Choreographer.FrameCallback {
    private final Handler handler = new Handler(Looper.getMainLooper());
    private LatencyBenchmarkStats stats = new LatencyBenchmarkStats();
    private boolean running;
    private boolean measured;
    private long nextTimerMs;
    private long lastUpdateMs;
    private TextView results;
    private Button start;
    private final Runnable finishTest = this::stopTest;
    private final Runnable timer = new Runnable() {
        @Override public void run() {
            if (!running) return;
            long now = SystemClock.uptimeMillis();
            stats.add(LatencyBenchmarkStats.TIMER_DELAY, (now - nextTimerMs) * 1_000_000L);
            if (now - lastUpdateMs >= 500) {
                showResults();
                lastUpdateMs = now;
            }
            nextTimerMs = now + 100;
            handler.postAtTime(this, nextTimerMs);
        }
    };

    @Override protected void onCreate(Bundle savedInstanceState) {
        UiHelper.setLocale(this);
        super.onCreate(savedInstanceState);
        FrameLayout content = TopAppBar.setUpPage(this, R.string.latency_benchmark_title, this::finish);
        android.view.View benchmark = getLayoutInflater().inflate(R.layout.latency_benchmark, content, false);
        content.addView(benchmark);
        results = benchmark.findViewById(R.id.benchmark_results);
        start = benchmark.findViewById(R.id.benchmark_start);
        start.setOnClickListener(v -> {
            if (running) stopTest();
            else startTest();
        });
        benchmark.findViewById(R.id.benchmark_share).setOnClickListener(v -> showShareDialog());
        com.limelight.ui.SubPageFocus.apply(this, benchmark);
        UiHelper.notifyNewRootView(this);
        showResults();
    }

    private void startTest() {
        stats = new LatencyBenchmarkStats();
        running = true;
        measured = true;
        start.setText(R.string.latency_benchmark_stop);
        lastUpdateMs = SystemClock.uptimeMillis();
        nextTimerMs = lastUpdateMs + 100;
        handler.postAtTime(timer, nextTimerMs);
        handler.postDelayed(finishTest, 10000);
        Choreographer.getInstance().postFrameCallback(this);
        showResults();
    }

    private void stopTest() {
        if (!running) return;
        running = false;
        handler.removeCallbacks(timer);
        handler.removeCallbacks(finishTest);
        Choreographer.getInstance().removeFrameCallback(this);
        start.setText(R.string.latency_benchmark_start);
        showResults();
    }

    @Override public void doFrame(long frameTimeNanos) {
        if (!running) return;
        stats.frame(frameTimeNanos, System.nanoTime());
        Choreographer.getInstance().postFrameCallback(this);
    }

    private void recordInput(long eventTimeMs) {
        if (running) stats.add(LatencyBenchmarkStats.INPUT_AGE,
                (SystemClock.uptimeMillis() - eventTimeMs) * 1_000_000L);
    }

    @Override public boolean dispatchTouchEvent(MotionEvent event) {
        recordInput(event.getEventTime());
        return super.dispatchTouchEvent(event);
    }

    @Override public boolean dispatchGenericMotionEvent(MotionEvent event) {
        recordInput(event.getEventTime());
        return super.dispatchGenericMotionEvent(event);
    }

    @Override public boolean dispatchKeyEvent(KeyEvent event) {
        recordInput(event.getEventTime());
        return super.dispatchKeyEvent(event);
    }

    private float panelHz() {
        Display display = getWindowManager().getDefaultDisplay();
        return Build.VERSION.SDK_INT >= Build.VERSION_CODES.M ?
                display.getMode().getRefreshRate() : display.getRefreshRate();
    }

    private String audioProperty(String name) {
        return ((AudioManager) getSystemService(AUDIO_SERVICE)).getProperty(name);
    }

    private void showResults() {
        StringBuilder text = new StringBuilder(getString(R.string.latency_benchmark_display, panelHz()));
        String rate = audioProperty(AudioManager.PROPERTY_OUTPUT_SAMPLE_RATE);
        String burst = audioProperty(AudioManager.PROPERTY_OUTPUT_FRAMES_PER_BUFFER);
        String unknown = getString(R.string.latency_unavailable);
        text.append('\n').append(getString(R.string.latency_benchmark_audio,
                rate == null ? unknown : rate, burst == null ? unknown : burst));
        int[] labels = {R.string.latency_benchmark_frame, R.string.latency_benchmark_callback,
                R.string.latency_benchmark_input, R.string.latency_benchmark_timer};
        for (int metric = 0; metric < labels.length; metric++) {
            double[] summary = stats.summary(metric);
            text.append("\n\n").append(getString(labels[metric])).append(":\n");
            text.append(summary[0] == 0 ? unknown : getString(R.string.latency_values,
                    summary[1], summary[2], summary[3], (int) summary[0]));
        }
        results.setText(text);
    }

    private String exportResults() {
        DeviceResults results = new DeviceResults();
        results.appVersion = BuildConfig.VERSION_NAME;
        results.appBuild = BuildConfig.VERSION_CODE;
        results.manufacturer = Build.MANUFACTURER;
        results.model = Build.MODEL;
        results.codename = Build.DEVICE;
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            results.socManufacturer = Build.SOC_MANUFACTURER;
            results.socModel = Build.SOC_MODEL;
        }
        results.hardware = Build.HARDWARE;
        results.board = Build.BOARD;
        results.androidRelease = Build.VERSION.RELEASE;
        results.sdk = Build.VERSION.SDK_INT;
        results.gpu = GlPreferences.readPreferences(this).glRenderer;
        DecoderBenchmark.addTo(this, results);
        if (measured) {
            DeviceResults.Latency latency = new DeviceResults.Latency();
            latency.panelHz = panelHz();
            latency.audioSampleRate = DeviceResults.Latency.audioProperty(
                    audioProperty(AudioManager.PROPERTY_OUTPUT_SAMPLE_RATE));
            latency.audioFramesPerBuffer = DeviceResults.Latency.audioProperty(
                    audioProperty(AudioManager.PROPERTY_OUTPUT_FRAMES_PER_BUFFER));
            for (int metric = 0; metric < latency.metrics.length; metric++) {
                latency.metrics[metric] = stats.summary(metric);
            }
            results.latency = latency;
        }
        return results.toJson();
    }

    private void showShareDialog() {
        String json = exportResults();
        android.view.View view = getLayoutInflater().inflate(R.layout.dialog_device_results, null);
        ((TextView) view.findViewById(R.id.device_results_message)).setText(
                getString(R.string.latency_benchmark_share_message) + "\n\n" + json);
        androidx.appcompat.app.AlertDialog dialog = new NavigationDialogBuilder(this)
                .setTitle(R.string.latency_benchmark_share_title)
                .setView(view)
                .create();
        view.findViewById(R.id.device_results_share).setOnClickListener(button -> {
            dialog.dismiss();
            Intent intent = new Intent(Intent.ACTION_SEND).setType("text/plain")
                    .putExtra(Intent.EXTRA_SUBJECT, getString(R.string.latency_benchmark_share_title))
                    .putExtra(Intent.EXTRA_TEXT, json);
            startActivity(Intent.createChooser(intent, getString(R.string.latency_benchmark_share)));
        });
        view.findViewById(R.id.device_results_report_form).setOnClickListener(button -> {
            dialog.dismiss();
            copyResults(json);
            Toast.makeText(this, R.string.latency_benchmark_paste_hint, Toast.LENGTH_LONG).show();
            HelpLauncher.launchUrl(this, DeviceResults.REPORT_FORM_URL);
        });
        view.findViewById(R.id.device_results_copy).setOnClickListener(button -> {
            dialog.dismiss();
            copyResults(json);
            Toast.makeText(this, R.string.latency_benchmark_copied, Toast.LENGTH_SHORT).show();
        });
        UiHelper.showDialog(this, dialog);
    }

    private void copyResults(String json) {
        ClipboardManager clipboard = (ClipboardManager) getSystemService(CLIPBOARD_SERVICE);
        clipboard.setPrimaryClip(ClipData.newPlainText(getString(R.string.latency_benchmark_share_title), json));
    }

    @Override protected void onPause() {
        stopTest();
        super.onPause();
    }
}

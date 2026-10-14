package com.limelight;

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

import androidx.appcompat.app.AppCompatActivity;

import com.limelight.utils.LatencyBenchmarkStats;
import com.limelight.ui.TopAppBar;
import com.limelight.utils.UiHelper;

public class LatencyBenchmarkActivity extends AppCompatActivity implements Choreographer.FrameCallback {
    private final Handler handler = new Handler(Looper.getMainLooper());
    private LatencyBenchmarkStats stats = new LatencyBenchmarkStats();
    private boolean running;
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
        UiHelper.notifyNewRootView(this);
        showResults();
    }

    private void startTest() {
        stats = new LatencyBenchmarkStats();
        running = true;
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

    private void showResults() {
        Display display = getWindowManager().getDefaultDisplay();
        float panelHz = Build.VERSION.SDK_INT >= Build.VERSION_CODES.M ?
                display.getMode().getRefreshRate() : display.getRefreshRate();
        StringBuilder text = new StringBuilder(getString(R.string.latency_benchmark_display, panelHz));
        AudioManager audio = (AudioManager) getSystemService(AUDIO_SERVICE);
        String rate = audio.getProperty(AudioManager.PROPERTY_OUTPUT_SAMPLE_RATE);
        String burst = audio.getProperty(AudioManager.PROPERTY_OUTPUT_FRAMES_PER_BUFFER);
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

    @Override protected void onPause() {
        stopTest();
        super.onPause();
    }
}

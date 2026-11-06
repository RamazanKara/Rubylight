package com.limelight.demo;

import android.app.Activity;
import android.os.Build;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.SystemClock;
import android.util.Log;
import android.view.FrameMetrics;
import android.view.KeyEvent;
import android.view.Window;

import java.util.concurrent.ConcurrentLinkedQueue;

/**
 * How long a key press takes to reach the screen, for scripts/home-speed.py: from the key's event time to the end of the
 * first frame that began drawing after the window handled it. Focus moves while a key is handled, so that frame is the
 * one showing the move. A press that moved nothing (the end of a shelf) has no such frame and is not counted. Logs one
 * line per press under {@link #TAG}.
 */
final class DemoFocusLatency implements Window.OnFrameMetricsAvailableListener {
    private static final String TAG = "NjLatency";
    private static final long MS = 1_000_000L;
    /** "--ez focusLatency true" at the demo's launcher. */
    static boolean wanted;

    private static final class Press {
        final long eventNanos;
        final long handledNanos;
        volatile boolean moved;

        Press(long eventNanos, long handledNanos) {
            this.eventNanos = eventNanos;
            this.handledNanos = handledNanos;
        }
    }

    private final ConcurrentLinkedQueue<Press> pending = new ConcurrentLinkedQueue<>();
    private Press newest;

    DemoFocusLatency(Activity activity) {
        HandlerThread thread = new HandlerThread("focus-latency");
        thread.start();
        activity.getWindow().addOnFrameMetricsAvailableListener(this, new Handler(thread.getLooper()));
        activity.getWindow().getDecorView().getViewTreeObserver().addOnGlobalFocusChangeListener((before, after) -> {
            if (newest != null) newest.moved = true;
        });
    }

    void onKey(KeyEvent event) {
        if (event.getAction() != KeyEvent.ACTION_DOWN) return;
        newest = new Press(event.getEventTime() * MS, SystemClock.uptimeMillis() * MS);
        pending.add(newest);
    }

    @Override
    public void onFrameMetricsAvailable(Window window, FrameMetrics metrics, int dropCount) {
        if (pending.isEmpty() || Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return;
        long drawStart = metrics.getMetric(FrameMetrics.INTENDED_VSYNC_TIMESTAMP) + metrics.getMetric(FrameMetrics.UNKNOWN_DELAY_DURATION)
                + metrics.getMetric(FrameMetrics.INPUT_HANDLING_DURATION) + metrics.getMetric(FrameMetrics.ANIMATION_DURATION)
                + metrics.getMetric(FrameMetrics.LAYOUT_MEASURE_DURATION);
        long swap = metrics.getMetric(FrameMetrics.SWAP_BUFFERS_DURATION);
        long last = Build.VERSION.SDK_INT >= Build.VERSION_CODES.S ? Math.max(swap, metrics.getMetric(FrameMetrics.GPU_DURATION)) : swap;
        long end = drawStart + metrics.getMetric(FrameMetrics.DRAW_DURATION) + metrics.getMetric(FrameMetrics.SYNC_DURATION)
                + metrics.getMetric(FrameMetrics.COMMAND_ISSUE_DURATION) + last;
        Press press;
        while ((press = pending.peek()) != null && press.handledNanos <= drawStart) {
            pending.poll();
            if (!press.moved) continue;
            Log.i(TAG, "press toFrameMs=" + (end - press.eventNanos) / (double) MS
                    + " toHandledMs=" + (press.handledNanos - press.eventNanos) / (double) MS);
        }
    }
}

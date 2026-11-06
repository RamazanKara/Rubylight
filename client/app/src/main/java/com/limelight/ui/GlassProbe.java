package com.limelight.ui;

import android.app.Activity;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import android.view.FrameMetrics;
import android.view.Window;

import androidx.annotation.RequiresApi;
import androidx.core.content.res.ResourcesCompat;

import com.limelight.R;

import java.util.Arrays;

/**
 * Watches the first {@code tier.probe.frames} real frames a live tier draws. A p90 above {@code tier.probe.p90BudgetFraction}
 * of the frame budget steps the screen down one tier, and the verdict is kept for this app version. Idle screens draw
 * nothing and count nothing, so the probe simply finishes on a later scroll, or the next time.
 */
@RequiresApi(Build.VERSION_CODES.N)
public final class GlassProbe implements Window.OnFrameMetricsAvailableListener {
    /** What to do with the verdict; called on the main thread. */
    public interface Verdict {
        void stepDownTo(GlassTier tier);
    }

    private final Activity activity;
    private final GlassTier tier;
    private final Verdict verdict;
    private final long[] frames;
    private final float budgetFraction;
    private final Handler main = new Handler(Looper.getMainLooper());
    private int count;
    private boolean done;

    private GlassProbe(Activity activity, GlassTier tier, Verdict verdict) {
        this.activity = activity;
        this.tier = tier;
        this.verdict = verdict;
        frames = new long[activity.getResources().getInteger(R.integer.nj_tier_probe_frames)];
        budgetFraction = ResourcesCompat.getFloat(activity.getResources(), R.dimen.nj_tier_probe_p90_budget_fraction);
    }

    /** Starts watching, unless this tier has nothing to probe or this app version already has its verdict. */
    public static GlassProbe start(Activity activity, GlassTier tier, Verdict verdict) {
        if ((tier != GlassTier.LENS && tier != GlassTier.LIVE) || GlassTier.probed(activity)) return null;
        GlassProbe probe = new GlassProbe(activity, tier, verdict);
        activity.getWindow().addOnFrameMetricsAvailableListener(probe, new Handler(Looper.getMainLooper()));
        return probe;
    }

    /** After a step down: the lower tier is watched too, though a verdict is already kept. */
    public static GlassProbe continueAt(Activity activity, GlassTier tier, Verdict verdict) {
        if (tier != GlassTier.LIVE) return null;
        GlassProbe probe = new GlassProbe(activity, tier, verdict);
        activity.getWindow().addOnFrameMetricsAvailableListener(probe, new Handler(Looper.getMainLooper()));
        return probe;
    }

    /** Stops watching; what was seen so far is dropped. */
    public void stop() {
        if (done) return;
        done = true;
        activity.getWindow().removeOnFrameMetricsAvailableListener(this);
    }

    @Override
    public void onFrameMetricsAvailable(Window window, FrameMetrics metrics, int dropCount) {
        if (done || metrics.getMetric(FrameMetrics.FIRST_DRAW_FRAME) == 1) return;
        frames[count++] = work(metrics);
        if (count < frames.length) return;
        stop();
        float refresh = activity.getWindowManager().getDefaultDisplay().getRefreshRate();
        long budget = (long) (1e9f / Math.max(refresh, 1f));
        // Removing the listener during its own callback is not allowed on every version: finish on the next turn.
        boolean fail = failed(frames, budget, budgetFraction);
        com.limelight.LimeLog.info("Glass probe at " + tier + ": " + (fail ? "steps down" : "keeps it"));
        main.post(() -> {
            GlassTier next = fail ? tier.stepDown() : tier;
            GlassTier.remember(activity, next);
            if (fail) verdict.stepDownTo(next);
        });
    }

    /** The app's and the GPU's own work on the frame, without the wait for its vsync. */
    private static long work(FrameMetrics metrics) {
        long cpu = metrics.getMetric(FrameMetrics.ANIMATION_DURATION) + metrics.getMetric(FrameMetrics.LAYOUT_MEASURE_DURATION)
                + metrics.getMetric(FrameMetrics.DRAW_DURATION) + metrics.getMetric(FrameMetrics.SYNC_DURATION)
                + metrics.getMetric(FrameMetrics.COMMAND_ISSUE_DURATION) + metrics.getMetric(FrameMetrics.SWAP_BUFFERS_DURATION);
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            return Math.max(cpu, metrics.getMetric(FrameMetrics.GPU_DURATION));
        }
        return cpu;
    }

    /** Whether the frames' 90th percentile is over the allowed part of the budget. */
    static boolean failed(long[] frames, long budgetNanos, float fraction) {
        long[] sorted = frames.clone();
        Arrays.sort(sorted);
        int index = Math.min(sorted.length - 1, (int) Math.ceil(sorted.length * 0.9) - 1);
        return sorted[index] > budgetNanos * fraction;
    }
}

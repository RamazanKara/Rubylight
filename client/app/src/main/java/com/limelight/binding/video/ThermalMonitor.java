package com.limelight.binding.video;

import android.content.Context;
import android.os.Build;
import android.os.PowerManager;
import android.os.SystemClock;

/**
 * How close the phone is to thermal throttling, polled at most every two seconds because
 * Android rate-limits headroom queries. Throttled clocks show up as late frames, so the
 * stream backs off before that happens.
 */
public final class ThermalMonitor {
    public static final int NORMAL = 0;
    public static final int WARM = 1;
    public static final int HOT = 2;
    // Headroom 1.0 is where Android reports severe throttling; act a little before it.
    static final float WARM_HEADROOM = 0.75f;
    static final float HOT_HEADROOM = 0.9f;
    private static final long POLL_MS = 2000;
    private static final int FORECAST_SECONDS = 10;

    private final PowerManager powerManager;
    private long lastPollMs = -POLL_MS;
    private int level = NORMAL;
    private float headroom = Float.NaN;

    public ThermalMonitor(Context context) {
        powerManager = Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q ?
                (PowerManager) context.getSystemService(Context.POWER_SERVICE) : null;
    }

    public synchronized int level() {
        long now = SystemClock.uptimeMillis();
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q || powerManager == null ||
                now - lastPollMs < POLL_MS) return level;
        lastPollMs = now;
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                float forecast = powerManager.getThermalHeadroom(FORECAST_SECONDS);
                // NaN means unsupported or polled too soon; keep the last reading.
                if (!Float.isNaN(forecast)) headroom = forecast;
            }
            level = levelFor(headroom, powerManager.getCurrentThermalStatus());
        } catch (RuntimeException ignored) {
        }
        return level;
    }

    /** Last forecast headroom, or NaN when the phone does not report one. */
    public synchronized float headroom() {
        return headroom;
    }

    static int levelFor(float headroom, int status) {
        int fromStatus = status >= PowerManager.THERMAL_STATUS_SEVERE ? HOT :
                status >= PowerManager.THERMAL_STATUS_MODERATE ? WARM : NORMAL;
        int fromHeadroom = !(headroom >= 0) ? NORMAL : headroom >= HOT_HEADROOM ? HOT :
                headroom >= WARM_HEADROOM ? WARM : NORMAL;
        return Math.max(fromStatus, fromHeadroom);
    }
}

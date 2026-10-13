package com.limelight.binding.video;

import android.os.Build;
import android.view.Display;

public final class DisplayFrameRatePolicy {
    public static final long SAMPLE_INTERVAL_MS = 250;
    private static final long LOWER_CADENCE_HOLD_MS = 2000;
    private static final float CADENCE_TOLERANCE = 0.03f;

    private float vote;
    private float lastPanelMaxHz;
    private float lastStreamFps;
    private long lastSampleMs = -1;
    private float candidate;
    private long candidateSinceMs = -1;
    private boolean confirmedCadence;

    public static boolean useAdaptiveHints(int sdk, boolean vrr, boolean arrSupported) {
        return vrr && sdk >= 36 && arrSupported;
    }

    public float cadenceVote(float measuredFps, float streamFps, float panelMaxHz, boolean useArr, long nowMs) {
        if (panelMaxHz != lastPanelMaxHz || streamFps != lastStreamFps ||
                (lastSampleMs >= 0 && (nowMs <= lastSampleMs || nowMs - lastSampleMs > 2 * SAMPLE_INTERVAL_MS))) {
            vote = panelMaxHz;
            confirmedCadence = false;
            candidateSinceMs = -1;
        }
        lastPanelMaxHz = panelMaxHz;
        lastStreamFps = streamFps;
        lastSampleMs = nowMs;

        // Arrivals cannot identify static content or the host game's intended FPS. Only sustained
        // interactive cadence may lower the vote. 60 FPS is a latency policy, not a panel mode;
        // allow fractional rates and explicitly negotiated lower rates without rounding the hint.
        float motionFloor = Math.min(60, Math.min(streamFps, panelMaxHz)) * (1 - CADENCE_TOLERANCE);
        if (!useArr || !Float.isFinite(measuredFps) || measuredFps <= 0 || measuredFps < motionFloor) {
            vote = panelMaxHz;
            confirmedCadence = false;
            candidateSinceMs = -1;
            return vote;
        }

        float target = Math.min(measuredFps, panelMaxHz);
        if (confirmedCadence && target > vote * (1 + CADENCE_TOLERANCE)) {
            // Recover before waiting to learn the new cadence; never pace or buffer frames here.
            vote = panelMaxHz;
            confirmedCadence = false;
            candidateSinceMs = -1;
        }
        if (confirmedCadence && Math.abs(target - vote) <= vote * CADENCE_TOLERANCE) {
            candidateSinceMs = -1;
        } else if (candidateSinceMs < 0 || Math.abs(target - candidate) > candidate * CADENCE_TOLERANCE) {
            candidate = target;
            candidateSinceMs = nowMs;
        } else if (nowMs - candidateSinceMs >= LOWER_CADENCE_HOLD_MS) {
            vote = Math.max(candidate, target);
            confirmedCadence = true;
            candidateSinceMs = -1;
        }
        return vote;
    }

    public static float maxRefreshRate(Display display) {
        float maximum;
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            maximum = display.getMode().getRefreshRate();
            for (Display.Mode mode : display.getSupportedModes()) {
                maximum = Math.max(maximum, mode.getRefreshRate());
                // An LTPO panel resting at a low rate can list its top rate only as an alternative.
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                    for (float rate : mode.getAlternativeRefreshRates()) {
                        maximum = Math.max(maximum, rate);
                    }
                }
            }
        } else {
            maximum = display.getRefreshRate();
            for (float rate : display.getSupportedRefreshRates()) {
                maximum = Math.max(maximum, rate);
            }
        }
        return maximum;
    }

    public static int streamFrameRate(float panelMaxHz) {
        // Match the host's 1–1000 Hz launch range.
        return Math.max(1, Math.min(1000, Math.round(panelMaxHz)));
    }
}

package com.limelight.binding.video;

import android.os.Build;
import android.view.Display;

public final class DisplayFrameRatePolicy {
    public static boolean useAdaptiveRefreshRate(int sdk, boolean vrr, boolean arrSupported) {
        return vrr && sdk >= 36 && arrSupported;
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

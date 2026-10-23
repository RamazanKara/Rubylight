package com.limelight.binding.video;

import android.os.Build;
import android.view.Display;

public final class DisplayFrameRatePolicy {
    public static boolean useAdaptiveRefreshRate(int sdk, boolean vrr, boolean arrSupported) {
        return vrr && sdk >= 36 && arrSupported;
    }

    /**
     * Whether the surface's frame rate vote follows the content rate the host's frame times
     * show. It needs the setting, a host that announced clock sync (Rubylight; any other host
     * sends no usable frame times) and a surface that can vote seamlessly: API 30 or newer, a
     * SurfaceView, and neither ARR (which infers the cadence itself, vote 0) nor VRR's maximum
     * rate vote.
     */
    public static boolean followsContentRate(int sdk, boolean enabled, boolean hostAnnouncesClockSync,
            boolean arr, boolean vrr, boolean textureView) {
        return enabled && hostAnnouncesClockSync && sdk >= Build.VERSION_CODES.R && !arr && !vrr && !textureView;
    }

    /**
     * The rate to vote for content running at contentHz. With "Allow lower refresh rate" off
     * (mayReduce false) the vote never drops below baseHz, the vote the stream would make
     * without the content rate, so it cannot pull the panel under the mode that setting
     * keeps. It never exceeds the panel's maximum when that is known.
     */
    public static float contentVote(float contentHz, float baseHz, boolean mayReduce, float panelMaxHz) {
        float vote = mayReduce ? contentHz : Math.max(contentHz, baseHz);
        return panelMaxHz > 0 ? Math.min(vote, panelMaxHz) : vote;
    }

    /**
     * Decides when a changed content rate becomes a new vote: a rate within 3% of the last vote
     * is no change, and votes are at least 2 s apart, so a settling rate cannot make the panel
     * hop between modes. The rate itself is already steady (the shared content rate tracker
     * reports one only after it held for 2 s).
     */
    public static final class ContentRateVote {
        public static final long MIN_INTERVAL_MS = 2000;
        public static final float HYSTERESIS = 0.03f;

        private float voted;
        private long votedAtMs;
        private boolean everVoted;

        /** The rate in Hz to vote now, or 0 for no new vote. */
        public float next(long nowMs, int contentMilliHz) {
            if (contentMilliHz <= 0) return 0;
            float hz = contentMilliHz / 1000f;
            if (voted > 0 && Math.abs(hz - voted) <= voted * HYSTERESIS) return 0;
            if (everVoted && nowMs - votedAtMs < MIN_INTERVAL_MS) return 0;
            voted = hz;
            votedAtMs = nowMs;
            everVoted = true;
            return hz;
        }

        /** Something else replaced the vote (a new surface, another stream rate): vote again. */
        public void invalidate() {
            voted = 0;
        }
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

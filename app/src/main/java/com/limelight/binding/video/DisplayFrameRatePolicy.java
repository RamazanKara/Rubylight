package com.limelight.binding.video;

import android.os.Build;
import android.view.Display;

public final class DisplayFrameRatePolicy {
    public static boolean useAdaptiveRefreshRate(int sdk, boolean vrr, boolean arrSupported) {
        return vrr && sdk >= 36 && arrSupported;
    }

    public enum ArrState {
        /** VRR was not asked for. */
        NOT_REQUESTED,
        /** Android's automatic policy decides the refresh rate. */
        IN_USE,
        /**
         * VRR was asked for on Android before 16, which has no adaptive refresh API: the maximum
         * rate is voted; with a host's frame times, a multiple of the game's rate once it holds
         * one, where the screen reaches it without a blank (vrrContentVote).
         */
        ANDROID_TOO_OLD,
        /** Android 16 or newer, but this screen does not report ARR support: as above. */
        PANEL_WITHOUT_ARR
    }

    /** Why the stream does or does not run on ARR, so the overlay can name the reason for the fallback. */
    public static ArrState arrState(int sdk, boolean vrr, boolean arrSupported) {
        if (!vrr) return ArrState.NOT_REQUESTED;
        if (useAdaptiveRefreshRate(sdk, true, arrSupported)) return ArrState.IN_USE;
        return sdk < 36 ? ArrState.ANDROID_TOO_OLD : ArrState.PANEL_WITHOUT_ARR;
    }

    /**
     * Whether the renderer tracks the content rate, before the stream knows whether ARR serves
     * VRR. A SurfaceView only. VRR stands in for the setting: without ARR it is the only way the
     * panel can come down to a seamless multiple of the game's rate (vrrContentVote). Otherwise it needs the setting and "Allow lower
     * refresh rate", which the settings screen greys it without.
     */
    public static boolean tracksContentRate(boolean enabled, boolean reduceRefreshRate, boolean vrr,
            boolean textureView) {
        return !textureView && (vrr || (enabled && reduceRefreshRate));
    }

    /**
     * Whether the surface's frame rate vote follows the content rate the host's frame times
     * show. It needs the setting or VRR, a host that announced clock sync (Rubylight; any other
     * host sends no usable frame times) and a surface that can vote seamlessly: API 30 or newer,
     * a SurfaceView, and not ARR, whose stream rate vote leaves the cadence to Android. VRR
     * without ARR (Android before 16, a panel without ARR) follows the content rate with
     * vrrContentVote, which keeps the panel maximum unless a lower multiple is seamless.
     */
    public static boolean followsContentRate(int sdk, boolean enabled, boolean hostAnnouncesClockSync,
            boolean arr, boolean vrr, boolean textureView) {
        return (enabled || vrr) && hostAnnouncesClockSync && sdk >= Build.VERSION_CODES.R && !arr && !textureView;
    }

    /**
     * The content vote with VRR and no ARR: the highest whole multiple of the game's rate up to
     * the panel maximum, and only when the screen reaches it without a blank (seamlessHz, see
     * seamlessRates). Otherwise the panel maximum, VRR's base vote. So a 60 fps game on a 60/120
     * panel stays at 120, where a frame waits on an 8.3 ms grid instead of 16.7 ms; a 90 fps game
     * on a 90/120 panel gets 90; and a fixed panel never gets a vote it cannot follow.
     */
    public static float vrrContentVote(float contentHz, float panelMaxHz, float[] seamlessHz) {
        if (contentHz <= 0 || panelMaxHz <= 0) return panelMaxHz;
        // Within a frame rate's measuring error (59.94 against 60) a rate counts as reached.
        float tolerance = rateTolerance(panelMaxHz);
        int multiple = (int) Math.floor((panelMaxHz + tolerance) / contentHz);
        if (multiple < 1) return panelMaxHz;
        float vote = contentHz * multiple;
        if (vote >= panelMaxHz - tolerance) return panelMaxHz;
        for (float rate : seamlessHz) {
            if (Math.abs(rate - vote) <= rateTolerance(vote)) return vote;
        }
        return panelMaxHz;
    }

    private static float rateTolerance(float hz) {
        return Math.max(0.5f, 0.01f * hz);
    }

    /**
     * The rates the screen reaches from its current mode without a blank: the mode's own rate
     * and, from Android 12, its alternatives. Android 11 lists none, so only the current rate
     * counts there: a switch that might blank is never voted.
     */
    public static float[] seamlessRates(Display display) {
        if (display == null) return new float[0];
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M) return new float[] {display.getRefreshRate()};
        Display.Mode mode = display.getMode();
        float[] alternatives = Build.VERSION.SDK_INT >= Build.VERSION_CODES.S ?
                mode.getAlternativeRefreshRates() : new float[0];
        float[] rates = new float[alternatives.length + 1];
        rates[0] = mode.getRefreshRate();
        System.arraycopy(alternatives, 0, rates, 1, alternatives.length);
        return rates;
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
     * The vote under ARR: the stream rate, never above the panel maximum. An explicit rate,
     * not 0: Android 15 and newer hold games (Nightjar declares one) at 60 Hz unless they ask
     * for a frame rate, and a 0 vote clears the request.
     */
    public static float arrVote(float streamHz, float panelMaxHz) {
        if (streamHz <= 0) return panelMaxHz;
        return panelMaxHz > 0 ? Math.min(streamHz, panelMaxHz) : streamHz;
    }

    /**
     * Under ARR, whether the compositor shows about half the frames the decoder rendered while
     * the display runs at about half the stream rate or less: the sign of Android holding the
     * stream at half its rate (a 60 Hz cap on a 120 fps stream). Fed once a second; it flags
     * after 3 s in a row and clears on the first sample that is not half.
     */
    public static final class ArrHalfRateCheck {
        public static final long HOLD_MS = 3000;
        /** Below this the counts are too coarse to call half. */
        public static final float MIN_RENDERED_FPS = 40;
        public static final float LOW = 0.4f;
        public static final float HIGH = 0.6f;

        private long halfSinceMs = -1;
        private boolean flagged;

        /**
         * @param presentedFps the compositor-confirmed rate, -1 when the device does not report
         *                     it: then nothing is judged (the rendered rate cannot show a cap)
         * @param displayHz    the rate the display reports to the app, which shows a frame rate
         *                     override such as the 60 Hz game default; 0 when unknown
         * @param streamHz     the stream's frame rate; 0 when unknown
         */
        public static boolean isHalf(float renderedFps, float presentedFps, float displayHz, float streamHz) {
            if (presentedFps < 0 || renderedFps < MIN_RENDERED_FPS) return false;
            if (presentedFps < renderedFps * LOW || presentedFps > renderedFps * HIGH) return false;
            // Wi-Fi often delivers frames in pairs, and a screen at the full rate then shows only
            // the newer of each: half the frames, with nothing holding the screen down. Only a
            // display at about half the stream rate (a vsync interval about twice the frame
            // interval) is Android's cap.
            return displayHz <= 0 || streamHz <= 0 || displayHz <= streamHz * HIGH;
        }

        /** Takes one sample; returns whether the check is flagged now. */
        public boolean update(long nowMs, float renderedFps, float presentedFps, float displayHz, float streamHz) {
            if (!isHalf(renderedFps, presentedFps, displayHz, streamHz)) {
                halfSinceMs = -1;
                flagged = false;
                return false;
            }
            if (halfSinceMs < 0) halfSinceMs = nowMs;
            flagged = nowMs - halfSinceMs >= HOLD_MS;
            return flagged;
        }

        public boolean flagged() {
            return flagged;
        }

        /** A new surface, stream or stop: start over. */
        public void reset() {
            halfSinceMs = -1;
            flagged = false;
        }
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

        /**
         * Whether a content vote should be withdrawn now: the tracker no longer reports a rate
         * (the game stopped holding one) and the last vote is at least 2 s old. The caller then
         * votes the stream's base rate again. Ready to vote the next steady rate afterwards.
         */
        public boolean withdraw(long nowMs, int contentMilliHz) {
            if (contentMilliHz > 0 || voted <= 0 || nowMs - votedAtMs < MIN_INTERVAL_MS) return false;
            voted = 0;
            votedAtMs = nowMs;
            return true;
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

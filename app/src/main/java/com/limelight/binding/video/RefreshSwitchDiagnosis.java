package com.limelight.binding.video;

import android.content.Context;

import com.limelight.R;

import java.util.ArrayList;
import java.util.List;
import java.util.Locale;
import java.util.TreeSet;

/**
 * Why the screen did not move to the refresh rate the stream voted for. Android treats
 * Surface.setFrameRate() as a hint: the display manager first narrows the modes an app may reach
 * (the user's smooth or adaptive display setting, battery saver, heat), then the compositor picks a
 * mode among the rest, and a switch that would blank the screen needs both an app and a user
 * opt-in. None of that is reported back, so this works out the likeliest cause from what an app can
 * read: the supported modes, each mode's seamless alternatives, and a few system settings.
 * Nothing here is measured on a multi-rate panel; see docs/expected-refresh-switch.md.
 */
public final class RefreshSwitchDiagnosis {
    private RefreshSwitchDiagnosis() {}

    /** One display mode as Android lists it. Alternatives are the rates reachable without a blank. */
    public static final class PanelMode {
        public final int id;
        public final int width;
        public final int height;
        public final float hz;
        public final float[] alternatives;

        public PanelMode(int id, int width, int height, float hz, float[] alternatives) {
            this.id = id;
            this.width = width;
            this.height = height;
            this.hz = hz;
            this.alternatives = alternatives == null ? new float[0] : alternatives.clone();
        }

        boolean sameSize(PanelMode other) {
            return width == other.width && height == other.height;
        }

        boolean switchesSeamlesslyTo(float rate) {
            for (float alternative : alternatives) {
                if (close(alternative, rate)) return true;
            }
            return false;
        }
    }

    /** System state that can cap a vote. Zero or false means unknown or not in effect. */
    public static final class Conditions {
        public final boolean batterySaver;
        public final boolean thermalCritical;
        /** The user's peak refresh rate setting (Settings.System peak_refresh_rate), 0 when unreadable. */
        public final float peakSettingHz;

        public Conditions(boolean batterySaver, boolean thermalCritical, float peakSettingHz) {
            this.batterySaver = batterySaver;
            this.thermalCritical = thermalCritical;
            this.peakSettingHz = peakSettingHz;
        }

        public static final Conditions NONE = new Conditions(false, false, 0f);
    }

    public enum Cause {
        /** The screen runs at the vote or a multiple of it. */
        HOLDS,
        /** No mode of this screen size runs at the vote or a multiple of it. */
        NO_MODE,
        /** Every matching mode is a switch that would blank the screen, and the vote allowed seamless switches only. */
        NOT_SEAMLESS,
        /**
         * Every matching mode would blank the screen and the vote allowed that; Android does such a
         * switch only when the user allows Match content frame rate in the display settings.
         */
        NEEDS_MATCH_SETTING,
        /** The user's refresh rate setting limits apps below the vote. */
        USER_LIMIT,
        BATTERY_SAVER,
        THERMAL,
        /** A matching seamless mode exists and nothing readable explains why Android stayed. */
        UNEXPLAINED
    }

    public static final class Result {
        public final Cause cause;
        public final float voteHz;
        public final float activeHz;
        /** The rates this screen offers at its current size, ascending. */
        public final float[] rates;
        /** The limit that applies for USER_LIMIT, else 0. */
        public final float limitHz;

        Result(Cause cause, float voteHz, float activeHz, float[] rates, float limitHz) {
            this.cause = cause;
            this.voteHz = voteHz;
            this.activeHz = activeHz;
            this.rates = rates;
            this.limitHz = limitHz;
        }

        public boolean mismatch() {
            return cause != Cause.HOLDS;
        }

        /** The same fact for the log, in English, with the numbers a bug report needs. */
        public String logLine(boolean seamlessOnly) {
            return "Refresh switch: " + cause + ", vote " + format(voteHz, Locale.ROOT) + " Hz (" +
                    (seamlessOnly ? "seamless only" : "may blank") + "), screen " +
                    format(activeHz, Locale.ROOT) + " Hz, modes " + formatRates(rates, Locale.ROOT) +
                    (limitHz > 0 ? ", user limit " + format(limitHz, Locale.ROOT) + " Hz" : "");
        }
    }

    /** True when the screen runs at the vote or a whole multiple of it (59.94 against 60 holds). */
    public static boolean holds(float voteHz, float activeHz) {
        return voteHz > 0 && activeHz > 0 && multiple(voteHz, activeHz);
    }

    private static boolean multiple(float base, float rate) {
        int n = Math.max(1, Math.round(rate / base));
        return Math.abs(rate - n * base) <= Math.max(0.5f, 0.01f * n * base);
    }

    private static boolean close(float a, float b) {
        return Math.abs(a - b) < 0.01f;
    }

    /**
     * @param sdk the Android API level; below 31 no mode lists alternatives, so a blank switch
     *            cannot be told from an allowed one and is never blamed
     * @param seamlessOnly the vote asked for seamless switches only (the content rate vote), as
     *            opposed to one that may blank the screen; it decides whether a switch that would
     *            blank is the vote's own limit or waits for the user's Match content frame rate
     */
    public static Result diagnose(int sdk, float voteHz, boolean seamlessOnly, PanelMode active,
                                  List<PanelMode> modes, Conditions conditions) {
        TreeSet<Float> sizeRates = new TreeSet<>();
        List<PanelMode> candidates = new ArrayList<>();
        for (PanelMode mode : modes) {
            if (!mode.sameSize(active)) continue;
            sizeRates.add(mode.hz);
            if (multiple(voteHz, mode.hz)) candidates.add(mode);
        }
        sizeRates.add(active.hz);
        float[] rates = new float[sizeRates.size()];
        int i = 0;
        for (float rate : sizeRates) rates[i++] = rate;

        if (holds(voteHz, active.hz)) return new Result(Cause.HOLDS, voteHz, active.hz, rates, 0f);
        if (candidates.isEmpty()) return new Result(Cause.NO_MODE, voteHz, active.hz, rates, 0f);

        float lowest = Float.MAX_VALUE;
        for (PanelMode mode : candidates) lowest = Math.min(lowest, mode.hz);
        // Battery saver and critical heat hold apps to 60 Hz (the display manager's low power and
        // skin temperature votes), which only matters when the vote is above that.
        if (lowest > 60.5f && conditions.batterySaver) {
            return new Result(Cause.BATTERY_SAVER, voteHz, active.hz, rates, 0f);
        }
        if (lowest > 60.5f && conditions.thermalCritical) {
            return new Result(Cause.THERMAL, voteHz, active.hz, rates, 0f);
        }
        if (conditions.peakSettingHz > 0 && lowest > conditions.peakSettingHz + 0.5f) {
            return new Result(Cause.USER_LIMIT, voteHz, active.hz, rates, conditions.peakSettingHz);
        }
        if (sdk >= 31) {
            boolean anySeamless = false;
            for (PanelMode mode : candidates) {
                if (active.switchesSeamlesslyTo(mode.hz)) {
                    anySeamless = true;
                    break;
                }
            }
            if (!anySeamless) {
                return new Result(seamlessOnly ? Cause.NOT_SEAMLESS : Cause.NEEDS_MATCH_SETTING,
                        voteHz, active.hz, rates, 0f);
            }
        }
        return new Result(Cause.UNEXPLAINED, voteHz, active.hz, rates, 0f);
    }

    /**
     * Holds back a verdict until the mismatch has lasted: a mode switch takes a moment, and a
     * vote that just changed has not had its chance. A changed vote or cause starts the wait again.
     */
    public static final class Watcher {
        public static final long SETTLE_MS = 3000;

        private float watchedVote;
        private Cause watchedCause;
        private long sinceMs;

        /** The result to show now, or null when the screen holds or the mismatch is still fresh. */
        public Result observe(long nowMs, Result result) {
            if (result == null || !result.mismatch()) {
                watchedCause = null;
                return null;
            }
            if (watchedCause != result.cause || Math.abs(watchedVote - result.voteHz) > 0.01f) {
                watchedCause = result.cause;
                watchedVote = result.voteHz;
                sinceMs = nowMs;
                return null;
            }
            return nowMs - sinceMs >= SETTLE_MS ? result : null;
        }
    }

    public static String describe(Context context, Result result) {
        Locale locale = Locale.getDefault();
        String vote = format(result.voteHz, locale);
        String active = format(result.activeHz, locale);
        String rates = formatRates(result.rates, locale);
        switch (result.cause) {
            case NO_MODE:
                return context.getString(R.string.refresh_diag_no_mode, vote, rates, active);
            case NOT_SEAMLESS:
                return context.getString(R.string.refresh_diag_not_seamless, vote, active, rates);
            case NEEDS_MATCH_SETTING:
                return context.getString(R.string.refresh_diag_needs_match_setting, vote, active, rates);
            case USER_LIMIT:
                return context.getString(R.string.refresh_diag_user_limit, vote, active,
                        format(result.limitHz, locale));
            case BATTERY_SAVER:
                return context.getString(R.string.refresh_diag_battery_saver, vote, active);
            case THERMAL:
                return context.getString(R.string.refresh_diag_thermal, vote, active);
            case UNEXPLAINED:
                return context.getString(R.string.refresh_diag_unexplained, vote, active, rates);
            default:
                return "";
        }
    }

    static String format(float hz, Locale locale) {
        if (Math.abs(hz - Math.round(hz)) < 0.005f) return Integer.toString(Math.round(hz));
        return String.format(locale, "%.2f", hz);
    }

    static String formatRates(float[] rates, Locale locale) {
        StringBuilder text = new StringBuilder();
        for (float rate : rates) {
            if (text.length() != 0) text.append(", ");
            text.append(format(rate, locale));
        }
        return text.toString();
    }
}

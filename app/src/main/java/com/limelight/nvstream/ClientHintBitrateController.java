package com.limelight.nvstream;

/**
 * Client-hint bitrate over 0x553C rate changes, equal to nightjar-policy's client_hint::Bitrate
 * through the shared traces. The decisions are Moonlight V+'s local controller
 * (AdaptiveBitrateService.tickLocal and bitrateRange, balanced mode, GPL-3.0); beyond V+: no
 * decision without recent video, one request outstanding, a host cap lowers the ceiling, a
 * refusal stops it, and a standing network queue holds probes.
 */
public final class ClientHintBitrateController {
    public static final long COOLDOWN_MS = 2000;
    public static final long START_DELAY_MS = 3000;
    public static final int STABLE_SAMPLES = 5;
    public static final float QUEUE_HOLD_MS = 20;

    private int minKbps;
    private int maxKbps;
    private int limitKbps;
    private int currentKbps;
    private final long startedMs;
    private long lastChangeMs = -1;
    private int stableSamples;
    private int lossStreak;
    private boolean lastDown;
    private int pendingKbps;
    private boolean stopped;

    /** limitKbps is this client's own cap, 0 for none. */
    public ClientHintBitrateController(int initialKbps, int limitKbps, long nowMs) {
        int[] range = range(initialKbps);
        maxKbps = limitKbps > 0 ? Math.min(range[1], limitKbps) : range[1];
        minKbps = Math.min(range[0], maxKbps);
        this.limitKbps = limitKbps;
        currentKbps = initialKbps;
        startedMs = nowMs;
    }

    /** V+'s balanced range: 0.3x (at least 3 Mbps) to 2x (at most 150 Mbps), holding the start, within 0.5-800 Mbps. */
    public static int[] range(int initialKbps) {
        int baseline = Math.max(500, Math.min(800_000, initialKbps));
        int low = Math.max(3000, (int) (baseline * 0.3));
        int high = Math.min(150_000, baseline * 2);
        return new int[] {Math.max(500, Math.min(baseline, low)), Math.min(800_000, Math.max(baseline, high))};
    }

    /** Sample once per second. Returns the bitrate to request, pending until applied or refused, or 0 to keep it. */
    public int sample(long nowMs, boolean recentVideo, float lossPercent, float queueMs) {
        if (stopped || pendingKbps != 0) return 0;
        if (!recentVideo) {
            stableSamples = 0;
            lossStreak = 0;
            return 0;
        }
        if (nowMs < startedMs + START_DELAY_MS || (lastChangeMs >= 0 && nowMs < lastChangeMs + COOLDOWN_MS)) return 0;
        // Unknown (negative or NaN) loss is no decision, and no clean sample.
        if (!(lossPercent >= 0)) {
            stableSamples = 0;
            return 0;
        }
        double target = currentKbps;
        if (lossPercent > 5) {
            target = currentKbps * 0.7;
            stableSamples = 0;
            lossStreak++;
        } else if (lossPercent > 2) {
            if (++lossStreak >= 2) {
                target = currentKbps * 0.9;
                stableSamples = 0;
            }
        } else if (lossPercent > 0.5f) {
            lossStreak++;
            stableSamples = 0;
            if (lossStreak >= 4) target = currentKbps * 0.95;
        } else if (queueMs >= QUEUE_HOLD_MS) {
            lossStreak = 0;
            stableSamples = 0;
        } else {
            lossStreak = 0;
            if (++stableSamples >= STABLE_SAMPLES && currentKbps < maxKbps) {
                target = currentKbps * (lastDown ? 1.02 : 1.05);
                stableSamples = 0;
            }
        }
        int kbps = Math.max(minKbps, Math.min(maxKbps, (int) target));
        if (kbps == currentKbps) return 0;
        pendingKbps = kbps;
        return kbps;
    }

    /** The host's result for the pending request; below the request, the host capped it and the ceiling follows. */
    public void applied(int kbps, long nowMs) {
        if (pendingKbps == 0) return;
        if (kbps < pendingKbps) {
            limitKbps = kbps;
            maxKbps = Math.min(maxKbps, kbps);
            minKbps = Math.min(minKbps, maxKbps);
        }
        pendingKbps = 0;
        if (kbps != currentKbps) lastDown = kbps < currentKbps;
        currentKbps = kbps;
        lastChangeMs = nowMs;
    }

    /** The host refused the pending request: the controller stops for this stream. */
    public void refused() {
        pendingKbps = 0;
        stopped = true;
    }

    /** The stream runs at kbps the user set by hand: the range follows it, as a new start under the same caps. */
    public void overridden(int kbps, long nowMs) {
        int[] range = range(kbps);
        maxKbps = limitKbps > 0 ? Math.min(range[1], limitKbps) : range[1];
        minKbps = Math.min(range[0], maxKbps);
        currentKbps = kbps;
        lastChangeMs = nowMs;
        stableSamples = 0;
        lossStreak = 0;
        lastDown = false;
        pendingKbps = 0;
    }

    public int currentKbps() {
        return currentKbps;
    }

    public int minKbps() {
        return minKbps;
    }

    public int maxKbps() {
        return maxKbps;
    }

    public boolean pending() {
        return pendingKbps != 0;
    }

    public boolean stopped() {
        return stopped;
    }
}

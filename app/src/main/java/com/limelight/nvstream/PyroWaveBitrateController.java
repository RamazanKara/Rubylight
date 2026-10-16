package com.limelight.nvstream;

public final class PyroWaveBitrateController {
    public static final int MAX_RUNTIME_KBPS = StreamConfiguration.MAX_BITRATE_KBPS;
    private int ceilingKbps;
    private int currentKbps;
    private int floorKbps;
    private final float frameMs;
    private long lastChangeMs;
    private long healthySinceMs = -1;
    private int congestedSamples;
    private float previousLoss = -1;
    private float previousQueueMs = -1;
    private int thermalLevel;

    public PyroWaveBitrateController(int ceilingKbps, int currentKbps, int fps, long nowMs) {
        this.ceilingKbps = Math.min(MAX_RUNTIME_KBPS, ceilingKbps);
        this.currentKbps = Math.min(this.ceilingKbps, currentKbps);
        floorKbps = Math.min(10000, this.currentKbps);
        frameMs = 1000.0f / fps;
        lastChangeMs = nowMs;
    }

    // Sample once per second while no runtime request is pending. Loss includes recovered frames' missing records.
    public int sample(long nowMs, boolean recentVideo, boolean poorConnection, float lossPercent, float queueMs, float decodeMs) {
        return sample(nowMs, recentVideo, poorConnection, lossPercent, queueMs, decodeMs, 0);
    }

    public int sample(long nowMs, boolean recentVideo, boolean poorConnection, float lossPercent, float queueMs, float decodeMs, float jitterMs) {
        if (!recentVideo) {
            healthySinceMs = -1;
            previousLoss = previousQueueMs = -1;
            if (!poorConnection) {
                congestedSamples = 0;
                return 0;
            }
        }
        boolean congested = poorConnection || (recentVideo && (lossPercent >= 2 || decodeMs >= frameMs ||
                jitterMs >= Math.max(5, frameMs * 0.75f) ||
                queueMs >= Math.max(20, frameMs * 2) ||
                (previousLoss >= 0 && lossPercent >= 0.5f && lossPercent - previousLoss >= 0.25f) ||
                (previousQueueMs >= 0 && queueMs >= Math.max(8, frameMs * 0.75f) &&
                        queueMs - previousQueueMs >= 3)));
        previousLoss = lossPercent;
        previousQueueMs = queueMs;
        if (congested || (recentVideo && thermalLevel >= 2)) {
            healthySinceMs = -1;
            // Heat builds and fades slowly, so heat alone steps down less often and not as far.
            int floor = congested ? floorKbps : Math.max(floorKbps, ceilingKbps / 2);
            if (++congestedSamples >= 2 && nowMs - lastChangeMs >= (congested ? 3000 : 15000)) {
                int target = Math.max(floor, currentKbps * 85 / 100);
                return target < currentKbps ? target : 0;
            }
        } else {
            congestedSamples = 0;
            if (thermalLevel > 0 || !(lossPercent >= 0 && lossPercent < 0.1f) || !(queueMs >= 0 && queueMs < Math.max(4, frameMs / 2)) ||
                    !(decodeMs >= 0 && decodeMs < frameMs * 0.75f) ||
                    !(jitterMs >= 0 && jitterMs < Math.max(5, frameMs * 0.25f))) {
                healthySinceMs = -1;
            } else if (healthySinceMs == -1) {
                healthySinceMs = nowMs;
            } else if (nowMs - healthySinceMs >= 20000 && nowMs - lastChangeMs >= 20000) {
                int target = Math.min(ceilingKbps, currentKbps + Math.max(250, currentKbps / 50));
                return target > currentKbps ? target : 0;
            }
        }
        return 0;
    }

    /** ThermalMonitor level: warm holds the rate, hot steps it down. */
    public void setThermalLevel(int level) {
        thermalLevel = level;
    }

    public void applied(int requestedKbps, int appliedKbps, long nowMs) {
        if (appliedKbps < requestedKbps) ceilingKbps = Math.min(ceilingKbps, appliedKbps);
        currentKbps = Math.min(MAX_RUNTIME_KBPS, appliedKbps);
        floorKbps = Math.min(floorKbps, currentKbps);
        lastChangeMs = nowMs;
        healthySinceMs = -1;
        congestedSamples = 0;
        previousLoss = previousQueueMs = -1;
    }

    public void suspend() {
        healthySinceMs = -1;
        congestedSamples = 0;
        previousLoss = previousQueueMs = -1;
    }
}

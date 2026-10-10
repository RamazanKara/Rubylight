package com.limelight.nvstream;

public final class AdaptiveBitrateController {
    private int ceilingKbps;
    private int currentKbps;
    private int floorKbps;
    private final float frameMs;
    private long lastChangeMs;
    private long healthySinceMs = -1;
    private int poorSamples;
    private float previousQueueMs = -1;

    public AdaptiveBitrateController(int ceilingKbps, int currentKbps, int fps, long nowMs) {
        this.ceilingKbps = Math.min(500000, ceilingKbps);
        this.currentKbps = Math.min(currentKbps, this.ceilingKbps);
        floorKbps = Math.min(2000, this.currentKbps);
        frameMs = 1000.0f / fps;
        lastChangeMs = nowMs;
    }

    // Called once per second, only while no bitrate request is in flight.
    public int sample(long nowMs, boolean recentVideo, boolean poorConnection, float lossPercent, float decodeMs) {
        return sample(nowMs, recentVideo, poorConnection, lossPercent, decodeMs, 0);
    }

    public int sample(long nowMs, boolean recentVideo, boolean poorConnection, float lossPercent, float decodeMs, float jitterMs) {
        return sample(nowMs, recentVideo, poorConnection, lossPercent, decodeMs, jitterMs, -1);
    }

    // queueMs is the network queueing delay above its recent minimum (-1 if unknown). A queue
    // building at the access point shows there before packets are dropped, so a rising queue
    // lowers the bitrate (and with it the host's pacing rate) before losses pile up.
    public int sample(long nowMs, boolean recentVideo, boolean poorConnection, float lossPercent, float decodeMs,
                      float jitterMs, float queueMs) {
        if (!recentVideo && !poorConnection) {
            healthySinceMs = -1;
            poorSamples = 0;
            previousQueueMs = -1;
            return 0;
        }
        boolean queueBuilding = recentVideo && queueMs >= 0 && (queueMs >= Math.max(20, frameMs * 2) ||
                (previousQueueMs >= 0 && queueMs >= Math.max(8, frameMs * 0.75f) && queueMs - previousQueueMs >= 3));
        previousQueueMs = recentVideo ? queueMs : -1;
        if (poorConnection || queueBuilding || (recentVideo && (lossPercent >= 3 || decodeMs >= frameMs ||
                jitterMs >= Math.max(5, frameMs * 0.75f)))) {
            healthySinceMs = -1;
            poorSamples++;
            if (poorSamples >= 2 && nowMs - lastChangeMs >= 3000) {
                int target = Math.max(floorKbps, currentKbps * 80 / 100);
                return target < currentKbps ? target : 0;
            }
        } else {
            poorSamples = 0;
            if (!(lossPercent >= 0 && lossPercent < 0.5f) ||
                    !(decodeMs >= 0 && decodeMs < frameMs * 0.75f) ||
                    !(jitterMs >= 0 && jitterMs < Math.max(2, frameMs * 0.25f)) ||
                    (queueMs >= 0 && queueMs >= Math.max(4, frameMs / 2))) {
                healthySinceMs = -1;
            } else if (healthySinceMs == -1) {
                healthySinceMs = nowMs;
            } else if (nowMs - healthySinceMs >= 15000 && nowMs - lastChangeMs >= 15000) {
                int target = Math.min(ceilingKbps, currentKbps + Math.max(250, currentKbps / 20));
                return target > currentKbps ? target : 0;
            }
        }
        return 0;
    }

    public void applied(int requestedKbps, int appliedKbps, long nowMs) {
        // The host may enforce a lower cap. Do not repeatedly probe beyond it.
        if (appliedKbps < requestedKbps) {
            ceilingKbps = Math.min(ceilingKbps, appliedKbps);
        }
        currentKbps = Math.min(500000, appliedKbps);
        floorKbps = Math.min(floorKbps, appliedKbps);
        lastChangeMs = nowMs;
        healthySinceMs = -1;
        poorSamples = 0;
        previousQueueMs = -1;
    }

    public void suspend() {
        healthySinceMs = -1;
        poorSamples = 0;
        previousQueueMs = -1;
    }
}

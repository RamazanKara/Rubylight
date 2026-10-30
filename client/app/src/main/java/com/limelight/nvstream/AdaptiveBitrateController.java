package com.limelight.nvstream;

import java.util.Locale;

public final class AdaptiveBitrateController {
    /** Lowest rate the controller steps down to unless the person sets a higher minimum. */
    public static final int DEFAULT_FLOOR_KBPS = 2000;

    /** Video codec of the stream; a newer codec needs fewer bits per pixel for a watchable picture. */
    public enum Codec {
        // Bits per thousand pixels. HEVC 1080p60 lands at 8.7 Mbps and 720p30 stays under the 2 Mbps minimum.
        H264(100), HEVC(70), AV1(65);

        final int bitsPerKiloPixel;

        Codec(int bitsPerKiloPixel) {
            this.bitsPerKiloPixel = bitsPerKiloPixel;
        }
    }

    /**
     * The lowest rate that still leaves a usable picture at this size, rate and codec, never below
     * DEFAULT_FLOOR_KBPS. An unknown size gives the flat default.
     */
    public static int builtInFloorKbps(int width, int height, int fps, Codec codec) {
        if (width <= 0 || height <= 0 || fps <= 0 || codec == null) {
            return DEFAULT_FLOOR_KBPS;
        }
        long kbps = (long) width * height * fps * codec.bitsPerKiloPixel / 1_000_000L;
        return (int) Math.max(DEFAULT_FLOOR_KBPS, Math.min(kbps, StreamConfiguration.MAX_BITRATE_KBPS));
    }

    // A rate with no poor sample for this long counts as stable and is what recovery steps back to.
    private static final long STABLE_MS = 10000;
    // Healthy time before stepping back toward the stable rate, and the pause after a step back failed.
    private static final long STEP_BACK_HEALTHY_MS = 5000;
    private static final long STEP_BACK_RETRY_HEALTHY_MS = 15000;
    private static final long STEP_BACK_SPACING_MS = 3000;
    private static final int STEP_BACK_MIN_KBPS = 250;
    // Jitter or queueing that a cut did not lower to this share of what triggered it was not helped by it.
    private static final float CUT_HELPED_RATIO = 0.85f;
    // This many cuts in a row that did not help stop further cuts for jitter and queueing.
    private static final int INEFFECTIVE_CUTS_TO_HOLD = 2;
    private static final long INSENSITIVE_CLEAR_MS = 5000;

    private int ceilingKbps;
    private final int startKbps;
    private int currentKbps;
    private int floorKbps;
    // Lowest rate the host applied below the floor, so a recomputed floor never exceeds a host cap.
    private int hostFloorCapKbps = Integer.MAX_VALUE;
    private float frameMs;
    private int fps;
    private int width;
    private int height;
    private final Codec codec;
    private long lastChangeMs;
    private long healthySinceMs = -1;
    private int poorSamples;
    private int thermalLevel;
    private float previousQueueMs = -1;
    private float decodeBeforeReductionMs = -1;
    private int decodeBeforeReductionKbps;
    private boolean decodeBitrateInsensitive;
    // The last few poor samples of the current run: jitter and queue against their trigger
    // thresholds, and the queue delay itself (-1 if unknown).
    private static final int RUN_SAMPLES = 3;
    // A queue rising by this much per sample over the run is bitrate-caused growth, never held.
    private static final float QUEUE_RISE_MS_PER_SAMPLE = 2;
    private static final float QUEUE_RISE_MIN_MS = 3;
    private final float[] runSeverity = new float[RUN_SAMPLES];
    private final float[] runQueueMs = new float[RUN_SAMPLES];
    private int runCount;
    private float lastCutSeverity = -1;
    private int ineffectiveCuts;
    private boolean networkBitrateInsensitive;
    private long calmSinceMs = -1;
    private int stableKbps;
    // The rate a step back asked for; applied() with this request is that step back. Kept across suspend().
    private int stepBackTargetKbps;
    private int stepBackFromKbps = -1;
    private int stepBackFailures;
    // Why the last non-zero sample() result was chosen, for the log.
    private String lastReason = "";

    public AdaptiveBitrateController(int ceilingKbps, int currentKbps, int fps, long nowMs) {
        this(ceilingKbps, currentKbps, fps, nowMs, 0, 0, null);
    }

    /**
     * The ceiling is the rate the stream starts at. The floor follows the stream's size, rate and
     * codec (see builtInFloorKbps), but never goes above half the starting rate, so congestion can
     * still bring the rate down.
     */
    public AdaptiveBitrateController(int ceilingKbps, int currentKbps, int fps, long nowMs,
                                     int width, int height, Codec codec) {
        this.ceilingKbps = Math.min(StreamConfiguration.MAX_BITRATE_KBPS, ceilingKbps);
        this.currentKbps = Math.min(currentKbps, this.ceilingKbps);
        startKbps = Math.min(this.currentKbps, this.ceilingKbps);
        this.width = width;
        this.height = height;
        this.codec = codec;
        this.fps = fps;
        frameMs = 1000.0f / fps;
        lastChangeMs = nowMs;
        updateFloor();
    }

    private void updateFloor() {
        int nominal = Math.min(builtInFloorKbps(width, height, fps, codec), Math.max(DEFAULT_FLOOR_KBPS, startKbps / 2));
        floorKbps = Math.min(Math.min(nominal, Math.min(startKbps, ceilingKbps)), hostFloorCapKbps);
    }

    /** Why the last non-zero sample() result was chosen: "device heat", "jitter 14 ms", "healthy" and the like. */
    public String lastReason() {
        return lastReason;
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
            calmSinceMs = -1;
            poorSamples = 0;
            resetPoorRun();
            resetHold();
            previousQueueMs = -1;
            return 0;
        }
        float jitterLimit = Math.max(5, frameMs * 0.75f);
        float queueLimit = Math.max(8, frameMs * 0.75f);
        boolean queueBuilding = recentVideo && queueMs >= 0 && (queueMs >= Math.max(20, frameMs * 2) ||
                (previousQueueMs >= 0 && queueMs >= queueLimit && queueMs - previousQueueMs >= 3));
        previousQueueMs = recentVideo ? queueMs : -1;
        // Loss the FEC could not repair and the native warning are congestion at any bitrate. Jitter
        // and queueing can come from the access point instead, whatever rate is sent.
        boolean hardPoor = poorConnection || (recentVideo && lossPercent >= 3);
        boolean poor = hardPoor || queueBuilding || (recentVideo && jitterMs >= jitterLimit);
        if (hardPoor) {
            networkBitrateInsensitive = false;
        }
        if (recentVideo && decodeMs >= 0 && decodeMs < frameMs * 0.75f) {
            decodeBitrateInsensitive = false;
            decodeBeforeReductionMs = -1;
        }
        boolean slowDecoder = recentVideo && decodeMs >= frameMs && !decodeBitrateInsensitive;
        if (slowDecoder && !poor && thermalLevel < 2 && decodeBeforeReductionMs >= 0 &&
                currentKbps < decodeBeforeReductionKbps && nowMs - lastChangeMs >= 15000 &&
                decodeMs >= decodeBeforeReductionMs * 0.95f) {
            // Fixed decoder buffering does not improve with bitrate. Stop cutting and allow recovery.
            decodeBitrateInsensitive = true;
            slowDecoder = false;
        }
        if (poor || (recentVideo && thermalLevel >= 2) || slowDecoder) {
            healthySinceMs = -1;
            calmSinceMs = -1;
            poorSamples++;
            if (poor) {
                addToRun(Math.max(jitterMs > 0 ? jitterMs / jitterLimit : 0, queueMs > 0 ? queueMs / queueLimit : 0),
                        recentVideo && queueMs >= 0 ? queueMs : -1);
            }
            // Heat and decoder pressure step down less often and not as far as network congestion.
            int floor = poor ? cutFloorKbps() : Math.max(floorKbps, ceilingKbps / 2);
            if (poorSamples >= 2 && nowMs - lastChangeMs >= (poor ? 3000 : 15000)) {
                int target = poor ? networkTarget(nowMs, hardPoor, floor) : Math.max(floor, currentKbps * 80 / 100);
                if (target < currentKbps && thermalLevel < 2 && slowDecoder) {
                    decodeBeforeReductionMs = decodeMs;
                    decodeBeforeReductionKbps = currentKbps;
                }
                if (target < currentKbps) {
                    lastReason = !poor ? (thermalLevel >= 2 ? "device heat" : String.format(Locale.ROOT, "decode %.1f ms", decodeMs))
                            : poorConnection ? "connection warning"
                            : recentVideo && lossPercent >= 3 ? String.format(Locale.ROOT, "%.1f%% frame loss", lossPercent)
                            : queueBuilding ? String.format(Locale.ROOT, "network queue %.0f ms", queueMs)
                            : String.format(Locale.ROOT, "jitter %.0f ms", jitterMs);
                    return target;
                }
                return 0;
            }
        } else {
            poorSamples = 0;
            resetPoorRun();
            if (calmSinceMs == -1) {
                calmSinceMs = nowMs;
            } else if (nowMs - calmSinceMs >= STABLE_MS) {
                // No poor sample for a while at this rate: remember it, and a step back has worked.
                stableKbps = Math.max(stableKbps, currentKbps);
                stepBackFromKbps = -1;
                stepBackFailures = 0;
            }
            if (thermalLevel > 0 || !(lossPercent >= 0 && lossPercent < 0.5f) ||
                    !(decodeMs >= 0 && (decodeBitrateInsensitive || decodeMs < frameMs * 0.75f)) ||
                    !(jitterMs >= 0 && jitterMs < Math.max(5, frameMs * 0.25f)) ||
                    (queueMs >= 0 && queueMs >= Math.max(4, frameMs / 2))) {
                healthySinceMs = -1;
            } else if (healthySinceMs == -1) {
                healthySinceMs = nowMs;
            } else {
                if (nowMs - healthySinceMs >= INSENSITIVE_CLEAR_MS) {
                    // The jitter or queueing is gone; cuts may be judged afresh.
                    networkBitrateInsensitive = false;
                    lastCutSeverity = -1;
                    ineffectiveCuts = 0;
                }
                int stable = Math.min(stableKbps, ceilingKbps);
                if (stable > currentKbps) {
                    // Trouble cut the rate from a stable one; come back in big steps instead of 5% per 15 s.
                    long healthyNeeded = stepBackFailures > 0 ? STEP_BACK_RETRY_HEALTHY_MS : STEP_BACK_HEALTHY_MS;
                    if (nowMs - healthySinceMs >= healthyNeeded && nowMs - lastChangeMs >= STEP_BACK_SPACING_MS) {
                        int gap = stable - currentKbps;
                        int step = Math.max((gap + 1) / 2, Math.min(gap, STEP_BACK_MIN_KBPS));
                        stepBackTargetKbps = currentKbps + step;
                        stepBackFromKbps = currentKbps;
                        lastReason = "step back toward " + stable + " kbps";
                        return stepBackTargetKbps;
                    }
                } else if (nowMs - healthySinceMs >= 15000 && nowMs - lastChangeMs >= 15000) {
                    int target = Math.min(ceilingKbps, currentKbps + Math.max(250, currentKbps / 20));
                    lastReason = "healthy";
                    return target > currentKbps ? target : 0;
                }
            }
        }
        return 0;
    }

    // The rate to cut to for a poor network sample, or the current rate to hold it.
    private int networkTarget(long nowMs, boolean hardPoor, int floor) {
        if (stepBackFromKbps > 0 && stepBackFromKbps < currentKbps && nowMs - lastChangeMs < STABLE_MS) {
            // The step back brought the trouble again: return to the rate that was fine and
            // remember that the stable rate lies between the two.
            int between = (stepBackFromKbps + currentKbps) / 2;
            stableKbps = between - stepBackFromKbps < STEP_BACK_MIN_KBPS ? stepBackFromKbps : between;
            stepBackFailures++;
            int from = stepBackFromKbps;
            stepBackFromKbps = -1;
            return Math.max(floor, from);
        }
        int cut = Math.max(floor, currentKbps * 80 / 100);
        if (cut >= currentKbps) {
            return currentKbps;
        }
        if (hardPoor) {
            lastCutSeverity = -1;
            ineffectiveCuts = 0;
            return cut;
        }
        float severity = runSeverity();
        if (queueRising()) {
            // A queue that keeps growing is the stream outrunning the link; only a lower rate stops it,
            // even when the last cut slowed the growth without ending it.
            networkBitrateInsensitive = false;
            ineffectiveCuts = 0;
            lastCutSeverity = severity;
            return cut;
        }
        if (networkBitrateInsensitive) {
            return currentKbps;
        }
        // Only jitter or a queue that is not growing: judge whether the previous cut lowered it.
        // Cutting further for a signal that does not follow the bitrate only ratchets the picture
        // down to the floor.
        if (lastCutSeverity >= 0) {
            ineffectiveCuts = severity > lastCutSeverity * CUT_HELPED_RATIO ? ineffectiveCuts + 1 : 0;
        }
        if (ineffectiveCuts >= INEFFECTIVE_CUTS_TO_HOLD) {
            networkBitrateInsensitive = true;
            return currentKbps;
        }
        lastCutSeverity = severity;
        return cut;
    }

    private void resetPoorRun() {
        runCount = 0;
    }

    private void addToRun(float severity, float queueMs) {
        if (runCount == RUN_SAMPLES) {
            System.arraycopy(runSeverity, 1, runSeverity, 0, RUN_SAMPLES - 1);
            System.arraycopy(runQueueMs, 1, runQueueMs, 0, RUN_SAMPLES - 1);
            runCount--;
        }
        runSeverity[runCount] = severity;
        runQueueMs[runCount] = queueMs;
        runCount++;
    }

    // Mean over the last poor samples, those just before the decision rather than the ones right
    // after the previous change.
    private float runSeverity() {
        float sum = 0;
        for (int i = 0; i < runCount; i++) sum += runSeverity[i];
        return sum / Math.max(1, runCount);
    }

    private boolean queueRising() {
        if (runCount < 2 || runQueueMs[0] < 0 || runQueueMs[runCount - 1] < 0) return false;
        float rise = runQueueMs[runCount - 1] - runQueueMs[0];
        return rise >= QUEUE_RISE_MIN_MS && rise / (runCount - 1) >= QUEUE_RISE_MS_PER_SAMPLE;
    }

    // What cuts judged earlier no longer say anything: the frame rate changed or video stopped.
    private void resetHold() {
        networkBitrateInsensitive = false;
        ineffectiveCuts = 0;
        lastCutSeverity = -1;
    }

    // A floor raised above the current rate (a larger stream size after congestion cut the rate)
    // only bounds cuts that start above it; loss must still be able to cut, down to the default floor.
    private int cutFloorKbps() {
        return floorKbps <= currentKbps ? floorKbps : Math.min(DEFAULT_FLOOR_KBPS, currentKbps);
    }

    /** ThermalMonitor level: warm holds the rate, hot steps it down. */
    public void setThermalLevel(int level) {
        thermalLevel = level;
    }

    public void setFrameRate(int fps) {
        this.fps = fps;
        frameMs = 1000.0f / fps;
        updateFloor();
        suspend();
        resetHold();
        decodeBeforeReductionMs = -1;
        decodeBitrateInsensitive = false;
    }

    /** The stream changed size (Automatic quality); the built-in floor follows it. */
    public void setStreamSize(int width, int height) {
        this.width = width;
        this.height = height;
        updateFloor();
    }

    /** Prefer bitrate reductions before lowering stream resolution for network congestion. */
    public boolean hasRoomBeforeQualityReduction() {
        return ceilingKbps <= 0 || currentKbps > Math.max(floorKbps, ceilingKbps * 65 / 100);
    }

    public void applied(int requestedKbps, int appliedKbps, long nowMs) {
        // The host may enforce a lower cap. Do not repeatedly probe beyond it.
        if (appliedKbps < requestedKbps) {
            ceilingKbps = Math.min(ceilingKbps, appliedKbps);
            hostFloorCapKbps = Math.min(hostFloorCapKbps, appliedKbps);
        }
        boolean raised = appliedKbps > currentKbps;
        boolean steppedBack = stepBackTargetKbps > 0 && requestedKbps == stepBackTargetKbps && raised;
        stepBackTargetKbps = 0;
        currentKbps = Math.min(StreamConfiguration.MAX_BITRATE_KBPS, appliedKbps);
        updateFloor();
        lastChangeMs = nowMs;
        // Steps back follow each other without waiting for a fresh healthy window.
        if (!steppedBack || currentKbps >= Math.min(stableKbps, ceilingKbps)) {
            healthySinceMs = -1;
        }
        if (!steppedBack) {
            stepBackFromKbps = -1;
        }
        if (raised) {
            lastCutSeverity = -1;
            ineffectiveCuts = 0;
        }
        calmSinceMs = -1;
        poorSamples = 0;
        resetPoorRun();
        previousQueueMs = -1;
    }

    public void suspend() {
        healthySinceMs = -1;
        calmSinceMs = -1;
        poorSamples = 0;
        resetPoorRun();
        previousQueueMs = -1;
    }
}

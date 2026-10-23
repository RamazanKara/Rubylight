package com.limelight.nvstream;

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

    // A request to come down to a person's maximum may repeat this often, like any other change.
    private static final long LIMIT_RETRY_MS = 3000;
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
    private final int maxLimitKbps;
    // Set only when the maximum lowered the ceiling, so a host rounding above the ceiling is left alone.
    private final boolean maximumLowersCeiling;
    private final int minLimitKbps;
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
    // Jitter and queue seen over the current run of poor samples, against their trigger thresholds.
    private float severitySum;
    private int severityCount;
    private float lastCutSeverity = -1;
    private int ineffectiveCuts;
    private boolean networkBitrateInsensitive;
    private long calmSinceMs = -1;
    private int stableKbps;
    private boolean stepBackPending;
    private int stepBackFromKbps = -1;
    private int stepBackFailures;

    public AdaptiveBitrateController(int ceilingKbps, int currentKbps, int fps, long nowMs) {
        this(ceilingKbps, currentKbps, fps, nowMs, 0, 0);
    }

    public AdaptiveBitrateController(int ceilingKbps, int currentKbps, int fps, long nowMs, int minKbps, int maxKbps) {
        this(ceilingKbps, currentKbps, fps, nowMs, minKbps, maxKbps, 0, 0, null);
    }

    /**
     * minKbps and maxKbps are the person's limits for the automatic rate, 0 for none. The minimum
     * replaces the built-in floor and the maximum lowers the ceiling, but neither is raised above
     * what the host allows. A minimum above the maximum yields to the minimum. Without a minimum the
     * floor follows the stream's size, rate and codec (see builtInFloorKbps), but never goes above
     * half the starting rate, so congestion can still bring the rate down.
     */
    public AdaptiveBitrateController(int ceilingKbps, int currentKbps, int fps, long nowMs, int minKbps, int maxKbps,
                                     int width, int height, Codec codec) {
        this.ceilingKbps = Math.min(StreamConfiguration.MAX_BITRATE_KBPS, ceilingKbps);
        this.currentKbps = Math.min(currentKbps, this.ceilingKbps);
        maxLimitKbps = maxKbps > 0 ? Math.max(maxKbps, minKbps) : 0;
        maximumLowersCeiling = maxLimitKbps > 0 && this.ceilingKbps > maxLimitKbps;
        if (maximumLowersCeiling) {
            // The stream may start above the maximum; sample() brings it down.
            this.currentKbps = Math.min(currentKbps, StreamConfiguration.MAX_BITRATE_KBPS);
            this.ceilingKbps = maxLimitKbps;
        }
        minLimitKbps = minKbps;
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
        int nominal;
        if (minLimitKbps > 0) {
            nominal = minLimitKbps;
        } else {
            nominal = Math.min(builtInFloorKbps(width, height, fps, codec), Math.max(DEFAULT_FLOOR_KBPS, startKbps / 2));
        }
        floorKbps = Math.min(Math.min(nominal, Math.min(startKbps, ceilingKbps)), hostFloorCapKbps);
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
        if (maximumLowersCeiling && currentKbps > ceilingKbps && nowMs - lastChangeMs >= LIMIT_RETRY_MS) {
            return ceilingKbps;
        }
        if (!recentVideo && !poorConnection) {
            healthySinceMs = -1;
            calmSinceMs = -1;
            resetPoorRun();
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
                severitySum += Math.max(jitterMs > 0 ? jitterMs / jitterLimit : 0, queueMs > 0 ? queueMs / queueLimit : 0);
                severityCount++;
            }
            // Heat and decoder pressure step down less often and not as far as network congestion.
            int floor = poor ? floorKbps : Math.max(floorKbps, ceilingKbps / 2);
            if (poorSamples >= 2 && nowMs - lastChangeMs >= (poor ? 3000 : 15000)) {
                int target = poor ? networkTarget(nowMs, hardPoor, floor) : Math.max(floor, currentKbps * 80 / 100);
                if (target < currentKbps && thermalLevel < 2 && slowDecoder) {
                    decodeBeforeReductionMs = decodeMs;
                    decodeBeforeReductionKbps = currentKbps;
                }
                return target < currentKbps ? target : 0;
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
                        stepBackPending = true;
                        stepBackFromKbps = currentKbps;
                        return currentKbps + step;
                    }
                } else if (nowMs - healthySinceMs >= 15000 && nowMs - lastChangeMs >= 15000) {
                    int target = Math.min(ceilingKbps, currentKbps + Math.max(250, currentKbps / 20));
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
        if (networkBitrateInsensitive) {
            return currentKbps;
        }
        // Only jitter or queueing: judge whether the previous cut lowered it. Cutting further for a
        // signal that does not follow the bitrate only ratchets the picture down to the floor.
        float severity = severitySum / Math.max(1, severityCount);
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
        severitySum = 0;
        severityCount = 0;
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
        }
        boolean raised = appliedKbps > currentKbps;
        boolean steppedBack = stepBackPending && raised;
        stepBackPending = false;
        if (appliedKbps < floorKbps) {
            hostFloorCapKbps = Math.min(hostFloorCapKbps, appliedKbps);
        }
        currentKbps = Math.min(StreamConfiguration.MAX_BITRATE_KBPS, appliedKbps);
        floorKbps = Math.min(floorKbps, appliedKbps);
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
        stepBackPending = false;
    }
}

package com.limelight.binding.input;

/**
 * Spots a controller gyroscope that delivers samples but no data: some controller clones list an
 * Android gyroscope whose values stay exactly zero while its timestamps advance. A real gyroscope
 * is noisy even at rest, so a full second of bit-for-bit zeros means the sensor is a phantom.
 * Ported from Moonlight V+ (qiin2333/moonlight-vplus, GPL-3.0), ControllerGyroLivenessTracker.
 */
final class GyroLivenessTracker {
    static final long DEFAULT_ZERO_NANOS = 1_000_000_000L;

    private final long zeroNanos;
    private long zeroRunStartNanos;
    private boolean tripped;

    GyroLivenessTracker() {
        this(DEFAULT_ZERO_NANOS);
    }

    GyroLivenessTracker(long zeroNanos) {
        this.zeroNanos = zeroNanos;
    }

    /** True once, for the sample that completes the zero run. */
    synchronized boolean onSample(float x, float y, float z, long timestampNanos) {
        if (tripped) {
            return false;
        }
        if (x != 0f || y != 0f || z != 0f) {
            zeroRunStartNanos = 0;
            return false;
        }
        if (zeroRunStartNanos == 0 || timestampNanos < zeroRunStartNanos) {
            zeroRunStartNanos = timestampNanos;
            return false;
        }
        if (timestampNanos - zeroRunStartNanos < zeroNanos) {
            return false;
        }
        tripped = true;
        return true;
    }
}

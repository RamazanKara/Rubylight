package com.limelight.nvstream;

/**
 * The host's answer to a 0x553C rate change (rubylight_protocol::rate_change::RateChangeResult):
 * the request's serial, the stream's bitrate and rate after it, and per value a status and a
 * reason in wire order.
 */
public final class RateChangeResult {
    /** Not asked for: the value stays. */
    public static final int STATUS_UNCHANGED = 0;
    /** The stream runs at the asked value. */
    public static final int STATUS_APPLIED = 1;
    /** The stream runs at a lower value than asked; the reason says what limits it. */
    public static final int STATUS_CLAMPED = 2;
    /** The stream keeps the value it had. */
    public static final int STATUS_REFUSED = 3;

    public static final int REASON_NONE = 0;
    public static final int REASON_INVALID = 1;
    public static final int REASON_DISABLED = 2;
    public static final int REASON_CODEC = 3;
    public static final int REASON_HOST_LIMIT = 4;
    public static final int REASON_ENCODER_LIMIT = 5;
    public static final int REASON_ENCODER_FAILED = 6;
    public static final int REASON_SUPERSEDED = 7;
    public static final int REASON_OTHER = 255;

    public final int serial;
    public final int bitrateKbps;
    public final int fpsMillihz;
    public final int bitrateStatus;
    public final int bitrateReason;
    public final int fpsStatus;
    public final int fpsReason;

    public RateChangeResult(int serial, int bitrateKbps, int fpsMillihz, int bitrateStatus,
                            int bitrateReason, int fpsStatus, int fpsReason) {
        this.serial = serial;
        this.bitrateKbps = bitrateKbps;
        this.fpsMillihz = fpsMillihz;
        this.bitrateStatus = bitrateStatus;
        this.bitrateReason = bitrateReason;
        this.fpsStatus = fpsStatus;
        this.fpsReason = fpsReason;
    }

    /** The stream now runs at the asked bitrate or a clamped one. */
    public boolean bitrateChanged() {
        return bitrateStatus == STATUS_APPLIED || bitrateStatus == STATUS_CLAMPED;
    }

    /** The stream now runs at the asked rate or a clamped one. */
    public boolean fpsChanged() {
        return fpsStatus == STATUS_APPLIED || fpsStatus == STATUS_CLAMPED;
    }

    @Override
    public String toString() {
        return "serial " + Integer.toUnsignedString(serial) + ": bitrate " + bitrateKbps + " kbps (" +
                bitrateStatus + "/" + bitrateReason + "), rate " + fpsMillihz + " mHz (" + fpsStatus + "/" +
                fpsReason + ")";
    }
}

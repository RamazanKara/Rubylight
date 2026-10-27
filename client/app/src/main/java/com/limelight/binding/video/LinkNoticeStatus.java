package com.limelight.binding.video;

import java.util.Locale;

/**
 * The host's notice that it lowered the video bitrate because of the network (control message
 * 0x5538, protocol/core/src/control.rs LinkNotice). Only a lowered bitrate has a status; the host
 * returning to the requested rate clears it.
 */
public final class LinkNoticeStatus {
    /** Lowered, probing back up. */
    public static final int STATE_LIMITED = 1;
    /** At the lowest bitrate the host uses, and the network still loses video. */
    public static final int STATE_FLOOR = 2;

    public final int state;
    public final int ceilingKbps;
    public final int currentKbps;

    private LinkNoticeStatus(int state, int ceilingKbps, int currentKbps) {
        this.state = state;
        this.ceilingKbps = ceilingKbps;
        this.currentKbps = currentKbps;
    }

    /**
     * The status for the values bridgeClLinkNotice hands over (state as on the wire: 0 full, 1 limited,
     * 2 floor), or null when the host is at the requested bitrate, which clears the notice, or the
     * values cannot be a notice.
     */
    public static LinkNoticeStatus parse(int state, int ceilingKbps, int currentKbps) {
        if (state != STATE_LIMITED && state != STATE_FLOOR) return null;
        if (ceilingKbps <= 0 || currentKbps <= 0) return null;
        return new LinkNoticeStatus(state, ceilingKbps, currentKbps);
    }

    /** "15" or "4.5": whole Mbps from 10, one decimal below, as the host's own messages say it. */
    public String currentMbps(Locale locale) {
        double mbps = currentKbps / 1000.0;
        return mbps >= 10 ? String.format(locale, "%.0f", mbps) : String.format(locale, "%.1f", mbps);
    }
}

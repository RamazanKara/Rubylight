package com.limelight;

import androidx.annotation.StringRes;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.PairingManager;

/**
 * How this device offers Input-only on one PC. A host with its own Input-only mode lists a tile
 * for it and starts no video. Elsewhere, with the setting on, Nightjar joins with an ordinary
 * stream at the smallest size and drops its video and audio unseen; the host still sends them.
 */
final class InputOnlyPolicy {
    enum Mode {
        /** The host's own Input-only tile. */
        NATIVE,
        /** A small ordinary stream whose video and audio are discarded. */
        SMALL_STREAM,
        /** Not offered: the setting is off. */
        OFF
    }

    // The smallest stream every host accepts: 16:9, a frame rate GameStream offers, and the
    // lowest bitrate the settings allow.
    static final int SMALL_WIDTH = 640;
    static final int SMALL_HEIGHT = 360;
    static final int SMALL_FPS = 30;
    static final int SMALL_BITRATE_KBPS = 500;
    // Hosts add 20% forward error correction to video by default, and send stereo audio at the
    // normal quality the core asks for at this bitrate.
    static final int VIDEO_FEC_PERCENT = 20;
    static final int AUDIO_KBPS = 96;

    private InputOnlyPolicy() {}

    static Mode mode(boolean anyHostEnabled, HostCapabilities caps, boolean hostListsInputOnly) {
        if (hostListsInputOnly || caps.has(HostCapabilities.Feature.REMOTE_SESSION_TILES)) {
            return Mode.NATIVE;
        }
        return anyHostEnabled ? Mode.SMALL_STREAM : Mode.OFF;
    }

    /** Why this PC cannot take a small-stream Input-only session now, as a short reason; 0 when it can. */
    @StringRes
    static int smallStreamBlocker(ComputerDetails pc) {
        if (pc.pairState != PairingManager.PairState.PAIRED) {
            return HostCapabilities.Reason.PAIR.text;
        }
        if (!pc.hasPermission(ComputerDetails.PERMISSION_VIEW | ComputerDetails.PERMISSION_LAUNCH)) {
            return HostCapabilities.Reason.VIEW_PERMISSION.text;
        }
        if (!pc.hasPermission(ComputerDetails.PERMISSION_INPUT)) {
            return R.string.input_only_reason_input_permission;
        }
        return 0;
    }

    /** Whether a small-stream session for this game leaves the host's running game alone. */
    static boolean keepsRunningGame(boolean hostRunningGame, boolean thisGameRunning) {
        return !hostRunningGame || thisGameRunning;
    }

    /** What the small stream costs on the network at most, in kbit/s. */
    static int estimatedKbps() {
        return SMALL_BITRATE_KBPS * (100 + VIDEO_FEC_PERCENT) / 100 + AUDIO_KBPS;
    }

    /** {@link #estimatedKbps()} over an hour, in megabytes. */
    static int estimatedMegabytesPerHour() {
        return (int) ((long) estimatedKbps() * 3600 / 8 / 1000);
    }
}

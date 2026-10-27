package com.limelight.binding.audio;

import com.limelight.R;
import com.limelight.nvstream.jni.MoonBridge;

// What the microphone is doing this session, for the performance overlay and the stream menu.
public enum MicrophoneStatus {
    OFF(0),
    NO_PERMISSION(R.string.mic_status_no_permission),
    NOT_OFFERED(R.string.mic_status_not_offered),
    UNAVAILABLE(R.string.mic_status_unavailable),
    MUTED(R.string.mic_status_muted),
    SENDING(R.string.mic_status_sending);

    // The overlay line's text, or 0 when the microphone is off and nothing is shown.
    public final int label;

    MicrophoneStatus(int label) {
        this.label = label;
    }

    // enabled: the setting is on. permitted: RECORD_AUDIO is granted. nativeState: a
    // MoonBridge.MIC_STATE_* value. capturing: the capture thread is recording or waiting out a mute.
    public static MicrophoneStatus of(boolean enabled, boolean permitted, int nativeState,
                                      boolean capturing, boolean muted) {
        if (!enabled) {
            return OFF;
        }
        if (!permitted) {
            return NO_PERMISSION;
        }
        if (nativeState == MoonBridge.MIC_STATE_NOT_OFFERED) {
            return NOT_OFFERED;
        }
        if (nativeState != MoonBridge.MIC_STATE_READY || !capturing) {
            return UNAVAILABLE;
        }
        return muted ? MUTED : SENDING;
    }

    // Whether the stream menu offers a mute toggle.
    public boolean canMute() {
        return this == SENDING || this == MUTED;
    }
}

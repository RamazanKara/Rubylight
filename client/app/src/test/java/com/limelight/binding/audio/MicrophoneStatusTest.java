package com.limelight.binding.audio;

import com.limelight.R;
import com.limelight.nvstream.jni.MoonBridge;

import org.junit.Test;
import static org.junit.Assert.*;

public class MicrophoneStatusTest {
    @Test public void offShowsNothingWhateverTheHostDid() {
        for (int state = MoonBridge.MIC_STATE_OFF; state <= MoonBridge.MIC_STATE_READY; state++) {
            MicrophoneStatus status = MicrophoneStatus.of(false, true, state, true, false);
            assertEquals(MicrophoneStatus.OFF, status);
            assertEquals(0, status.label);
            assertFalse(status.canMute());
        }
    }

    @Test public void missingPermissionComesBeforeTheHost() {
        assertEquals(MicrophoneStatus.NO_PERMISSION,
                MicrophoneStatus.of(true, false, MoonBridge.MIC_STATE_READY, true, false));
        assertEquals(R.string.mic_status_no_permission, MicrophoneStatus.NO_PERMISSION.label);
    }

    @Test public void hostThatDoesNotOfferIsNamed() {
        MicrophoneStatus status = MicrophoneStatus.of(true, true, MoonBridge.MIC_STATE_NOT_OFFERED, false, false);
        assertEquals(MicrophoneStatus.NOT_OFFERED, status);
        assertEquals(R.string.mic_status_not_offered, status.label);
        assertFalse(status.canMute());
    }

    @Test public void failedSetupOrCaptureIsUnavailable() {
        assertEquals(MicrophoneStatus.UNAVAILABLE,
                MicrophoneStatus.of(true, true, MoonBridge.MIC_STATE_UNAVAILABLE, false, false));
        // Ready on the host side, but recording failed or never started.
        assertEquals(MicrophoneStatus.UNAVAILABLE,
                MicrophoneStatus.of(true, true, MoonBridge.MIC_STATE_READY, false, false));
        // Still negotiating.
        assertEquals(MicrophoneStatus.UNAVAILABLE,
                MicrophoneStatus.of(true, true, MoonBridge.MIC_STATE_OFF, false, false));
    }

    @Test public void sendingAndMutedCanBeToggled() {
        MicrophoneStatus sending = MicrophoneStatus.of(true, true, MoonBridge.MIC_STATE_READY, true, false);
        MicrophoneStatus muted = MicrophoneStatus.of(true, true, MoonBridge.MIC_STATE_READY, true, true);
        assertEquals(MicrophoneStatus.SENDING, sending);
        assertEquals(MicrophoneStatus.MUTED, muted);
        assertEquals(R.string.mic_status_sending, sending.label);
        assertEquals(R.string.mic_status_muted, muted.label);
        assertTrue(sending.canMute());
        assertTrue(muted.canMute());
    }

    @Test public void framesAreTwentyMillisecondsOfOpusAtFortyEightKilohertz() {
        assertEquals(48000, MicrophoneCapture.SAMPLE_RATE);
        assertEquals(960, MicrophoneCapture.FRAME_SAMPLES);
    }
}

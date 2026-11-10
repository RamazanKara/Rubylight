package com.limelight.binding.video;

import org.junit.Test;

import static com.limelight.binding.video.PackedFallback.Action.*;
import static org.junit.Assert.*;

public class PackedFallbackTest {
    @Test
    public void aFailureReconnectsWithoutPackedChromaOnceAndThenEndsTheStream() {
        assertEquals(RECONNECT_WITHOUT_PACKED, PackedFallback.onFailure(false, true));
        // Already reconnected without it: another failure is not about packing, so no second round.
        assertEquals(END_STREAM, PackedFallback.onFailure(true, true));
        // Nothing to reconnect with (background, finishing): the old behaviour, end the stream.
        assertEquals(END_STREAM, PackedFallback.onFailure(false, false));
    }

    @Test
    public void theSessionFlagTurnsTheSettingOffWithoutChangingIt() {
        assertTrue(PackedFallback.effective(true, false));
        assertFalse(PackedFallback.effective(true, true));
        assertFalse(PackedFallback.effective(false, false));
        assertFalse(PackedFallback.effective(false, true));
    }

    @Test
    public void theToastFitsOneLineAndTheReasonStaysInTheLog() {
        // Android 12+ shows two lines at most; the reason (an exception message at worst) is logged.
        assertTrue(PackedFallback.toast().length() <= 60);
        assertTrue(PackedFallback.toast().contains("without"));
        for (String stage : new String[] {"RTSP handshake", PackedFallback.CHECK_STAGE, null}) {
            assertTrue(PackedFallback.refusedToast(stage).length() <= 70);
            assertTrue(PackedFallback.refusedToast(stage).contains("without"));
        }
    }

    @Test
    public void aRefusalThisDeviceMadeIsNotBlamedOnThePc() {
        assertTrue(PackedFallback.refusedToast("RTSP handshake").contains("PC"));
        assertTrue(PackedFallback.refusedToast(null).contains("PC"));
        assertFalse(PackedFallback.refusedToast(PackedFallback.CHECK_STAGE).contains("PC"));
    }
}

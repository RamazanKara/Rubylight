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
    }
}

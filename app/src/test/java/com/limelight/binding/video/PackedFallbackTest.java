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
    public void theToastsAreSharperColoredTextsTranslatedNotices() {
        // Translated resources, named for the setting; the reason (an exception message at worst) is logged.
        assertEquals(com.limelight.R.string.sharp_color_stopped, PackedFallback.toast());
        assertEquals(com.limelight.R.string.sharp_color_unavailable_device, PackedFallback.refusedToast(PackedFallback.CHECK_STAGE));
        for (String stage : new String[] {"RTSP handshake", "full chroma offer", null}) {
            assertEquals(com.limelight.R.string.sharp_color_unavailable_pc, PackedFallback.refusedToast(stage));
        }
    }

    @Test
    public void aCheckThatNeverAnswersCountsAsFailedInsteadOfHoldingTheStart() throws Exception {
        assertTrue(PackedFallback.within(() -> true, 5000, "quick"));
        assertFalse(PackedFallback.within(() -> false, 5000, "quick"));
        java.util.concurrent.CountDownLatch release = new java.util.concurrent.CountDownLatch(1);
        long before = System.nanoTime();
        try {
            assertFalse(PackedFallback.within(() -> {
                try {
                    release.await();
                } catch (InterruptedException ignored) {
                }
                return true;
            }, 100, "stuck"));
        } finally {
            release.countDown();
        }
        assertTrue(System.nanoTime() - before < 5_000_000_000L);
    }

    @Test
    public void aRefusalThisDeviceMadeIsNotBlamedOnThePc() {
        assertEquals(com.limelight.R.string.sharp_color_unavailable_pc, PackedFallback.refusedToast("RTSP handshake"));
        assertEquals(com.limelight.R.string.sharp_color_unavailable_pc, PackedFallback.refusedToast(null));
        assertEquals(com.limelight.R.string.sharp_color_unavailable_device, PackedFallback.refusedToast(PackedFallback.CHECK_STAGE));
    }
}

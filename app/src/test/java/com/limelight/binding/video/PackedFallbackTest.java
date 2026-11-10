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

    // Nightjar #88: all of this was found out only after the game had been launched on the PC.
    @Test
    public void whatCannotRunIsKnownBeforeTheLaunch() {
        int[] probes = {0};
        java.util.function.IntSupplier all = () -> { probes[0]++; return 0b1111; };
        java.util.function.IntSupplier none = () -> { probes[0]++; return 0; };
        // The old engine never carries it, and a PC that offers nothing is not worth a decode.
        assertEquals(PackedFallback.Unavailable.ENGINE, PackedFallback.beforeLaunch(false, 0b1111, false, all));
        assertEquals(PackedFallback.Unavailable.HOST, PackedFallback.beforeLaunch(true, 0, false, all));
        assertEquals(PackedFallback.Unavailable.HOST, PackedFallback.beforeLaunch(true, -1, true, all));
        assertEquals(0, probes[0]);
        // A PC that does not say and was never refused is asked, as before.
        assertNull(PackedFallback.beforeLaunch(true, -1, false, all));
        // Serverinfo that names modes outranks an old refusal.
        assertNull(PackedFallback.beforeLaunch(true, 0b0001, true, all));
        assertEquals(PackedFallback.Unavailable.DEVICE, PackedFallback.beforeLaunch(true, 0b1111, false, none));
        assertEquals(PackedFallback.Unavailable.DEVICE, PackedFallback.beforeLaunch(true, -1, false, none));
        assertEquals(PackedFallback.Unavailable.NO_COMMON_MODE,
                PackedFallback.beforeLaunch(true, 0b1010, false, () -> 0b0101));
        assertNull(PackedFallback.beforeLaunch(true, 0b1010, false, () -> 0b0110));
    }

    @Test
    public void theReasonPicksWhoTheToastNames() {
        assertFalse(PackedFallback.refusedToast(PackedFallback.Unavailable.ENGINE.stage).contains("PC"));
        assertFalse(PackedFallback.refusedToast(PackedFallback.Unavailable.DEVICE.stage).contains("PC"));
        assertTrue(PackedFallback.refusedToast(PackedFallback.Unavailable.HOST.stage).contains("PC"));
        assertTrue(PackedFallback.refusedToast(PackedFallback.Unavailable.NO_COMMON_MODE.stage).contains("PC"));
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
        assertTrue(PackedFallback.refusedToast("RTSP handshake").contains("PC"));
        assertTrue(PackedFallback.refusedToast(null).contains("PC"));
        assertFalse(PackedFallback.refusedToast(PackedFallback.CHECK_STAGE).contains("PC"));
    }
}

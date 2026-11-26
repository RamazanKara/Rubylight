package com.limelight.binding.video;

import org.junit.Test;

import static org.junit.Assert.*;

/**
 * JNI round-trip checks for {@link CompositorWatch} backed by
 * {@code nightjar_policy::video_queue::CompositorWatch}.
 */
public class CompositorWatchTest {
    private static final long MS = 1_000_000L;
    private static final long SEC = 1_000_000_000L;
    private static final long FRAME_60 = 16_666_667L;

    private static void feedSecond(CompositorWatch watch, long startNs, boolean deadCallbacks, boolean judder) {
        long rel = startNs;
        long ren = startNs;
        for (int i = 0; i < 60; i++) {
            rel += FRAME_60;
            watch.onReleased(rel, true);
            if (!deadCallbacks) {
                ren += judder ? ((i % 2 == 0) ? 100_000L : FRAME_60 * 2L) : FRAME_60;
                watch.onRendered(rel, ren);
            }
        }
    }

    @Test
    public void healthyStreamStaysDormantAndCloseIsSafe() {
        try (CompositorWatch watch = new CompositorWatch()) {
            long now = 10 * SEC;
            for (int i = 0; i < 12; i++) {
                feedSecond(watch, now, false, false);
                now += SEC;
                assertEquals(CompositorWatch.ACTION_NONE, watch.poll(now, false, false));
            }
            assertFalse(watch.isLockedOn());
            assertNull(watch.takeEvent());
        }
    }

    @Test
    public void deadCallbacksRecoverAndLockOnViaJni() {
        try (CompositorWatch watch = new CompositorWatch()) {
            long now = 100 * SEC;
            assertEquals(CompositorWatch.ACTION_NONE, watch.poll(now, false, false));
            for (int i = 0; i < 2; i++) {
                feedSecond(watch, now, true, false);
                now += SEC;
                assertEquals(CompositorWatch.ACTION_NONE, watch.poll(now, false, false));
            }
            int probeAction = CompositorWatch.ACTION_NONE;
            for (int i = 0; i < 3; i++) {
                feedSecond(watch, now, true, false);
                now += SEC;
                probeAction = watch.poll(now, false, false);
            }
            assertEquals(CompositorWatch.ACTION_KEEP_ON, probeAction);
            assertNotNull(watch.takeEvent());

            feedSecond(watch, now, false, false);
            now += SEC;
            assertEquals(CompositorWatch.ACTION_NONE, watch.poll(now, false, false));
            assertTrue(watch.isLockedOn());
            assertNotNull(watch.takeEvent());
        }
    }

    @Test
    public void postHandoffJudderPulsesThenEscalatesAndLocksOn() {
        try (CompositorWatch watch = new CompositorWatch()) {
            long now = SEC;
            watch.poll(now, false, false);
            for (int i = 0; i < 3; i++) {
                feedSecond(watch, now, false, false);
                now += SEC;
                watch.poll(now, false, false);
            }
            int pulseAction = CompositorWatch.ACTION_NONE;
            for (int i = 0; i < 4; i++) {
                feedSecond(watch, now, false, true);
                now += SEC;
                pulseAction = watch.poll(now, false, false);
            }
            assertEquals(CompositorWatch.ACTION_PULSE, pulseAction);

            int keepOnAction = CompositorWatch.ACTION_NONE;
            for (int i = 0; i < 6; i++) {
                feedSecond(watch, now, false, true);
                now += SEC;
                int act = watch.poll(now, false, false);
                if (act != CompositorWatch.ACTION_NONE) keepOnAction = act;
            }
            assertEquals(CompositorWatch.ACTION_KEEP_ON, keepOnAction);

            for (int i = 0; i < 3; i++) {
                feedSecond(watch, now, false, false);
                now += SEC;
                assertEquals(CompositorWatch.ACTION_NONE, watch.poll(now, false, false));
            }
            assertTrue(watch.isLockedOn());
            watch.close();
            assertEquals(CompositorWatch.ACTION_NONE, watch.poll(now + SEC, false, false));
            assertFalse(watch.isLockedOn());
        }
    }
}

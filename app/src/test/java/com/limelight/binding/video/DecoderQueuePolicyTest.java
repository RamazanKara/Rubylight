package com.limelight.binding.video;

import org.junit.Test;
import static org.junit.Assert.*;

public class DecoderQueuePolicyTest {
    @Test public void oldErrataFamiliesReturnOutputBuffersPromptly() {
        for (String family : new String[] {"MTK", "Exynos", "Nvidia", "Brcm", "TI", "Allwinner", "Amlogic", "Intel"}) {
            assertEquals(family, 1, DecoderQueuePolicy.outputLimit("OMX." + family + ".avc.decoder", true, false, false));
        }
    }

    @Test public void modernFamiliesRequireAdvertisedSupportInsteadOfAssumedSocTimings() {
        for (String family : new String[] {"qti", "mtk", "exynos", "google", "hisi", "unknown"}) {
            assertEquals(1, DecoderQueuePolicy.outputLimit("c2." + family + ".av1.decoder", false, false, false));
            assertEquals(2, DecoderQueuePolicy.outputLimit("c2." + family + ".av1.decoder", true, false, false));
            assertEquals(1, DecoderQueuePolicy.outputLimit("c2." + family + ".av1.decoder", true, true, false));
            assertEquals(1, DecoderQueuePolicy.outputLimit("c2." + family + ".av1.decoder", true, false, true));
        }
    }

    private static final long CONTENT_NS = 11_111_111L;
    private static final long MS = 1_000_000L;

    /** Feeds count outputs, one per content interval, each with the given input-to-output time. */
    private static long feed(DecoderQueuePolicy.Residency r, int count, long i2oNs, long nowNs) {
        for (int i = 0; i < count; i++) {
            r.track(r.onOutput(i2oNs, nowNs), CONTENT_NS);
            nowNs += CONTENT_NS;
        }
        return nowNs;
    }

    @Test public void residencyAtTheMinimumNeverDrops() {
        DecoderQueuePolicy.Residency r = new DecoderQueuePolicy.Residency();
        feed(r, 500, 4 * MS, 1_000_000_000L);
        assertFalse(r.shouldDrop(3));
    }

    @Test public void residencyFirstOutputAndUnknownTimesHaveNoExcess() {
        DecoderQueuePolicy.Residency r = new DecoderQueuePolicy.Residency();
        assertEquals(0, r.onOutput(-1, 1_000_000_000L));
        assertEquals(0, r.onOutput(0, 1_000_000_000L));
        assertEquals("the first frame is its own minimum", 0, r.onOutput(9 * MS, 1_000_000_000L));
        assertEquals(3 * MS, r.onOutput(12 * MS, 1_000_000_000L + CONTENT_NS));
    }

    @Test public void residencyDropsAfterAStreakOfThirtyOutputsAboveHalfAnInterval() {
        DecoderQueuePolicy.Residency r = new DecoderQueuePolicy.Residency();
        long now = feed(r, 20, 4 * MS, 1_000_000_000L);
        // 4 ms is the floor; 4 + 6 ms is above half an interval (5.6 ms).
        for (int i = 0; i < DecoderQueuePolicy.Residency.FRAMES - 1; i++) {
            now = feed(r, 1, 10 * MS, now);
            assertFalse("after " + (i + 1), r.shouldDrop(2));
        }
        feed(r, 1, 10 * MS, now);
        assertTrue(r.shouldDrop(1));
        assertFalse("nothing behind it, nothing to drop for", r.shouldDrop(0));
    }

    @Test public void residencyStreakBreaksOnAnyOutputAtTheFloor() {
        DecoderQueuePolicy.Residency r = new DecoderQueuePolicy.Residency();
        long now = feed(r, 20, 4 * MS, 1_000_000_000L);
        now = feed(r, 29, 10 * MS, now);
        now = feed(r, 1, 4 * MS, now);
        now = feed(r, 29, 10 * MS, now);
        assertFalse(r.shouldDrop(5));
    }

    @Test public void residencyDropsAtMostOneInFour() {
        DecoderQueuePolicy.Residency r = new DecoderQueuePolicy.Residency();
        long now = feed(r, 20, 4 * MS, 1_000_000_000L);
        now = feed(r, 40, 10 * MS, now);
        assertTrue(r.shouldDrop(1));
        r.onDropped();
        for (int i = 1; i < DecoderQueuePolicy.Residency.PERIOD; i++) {
            now = feed(r, 1, 10 * MS, now);
            assertFalse("not yet, output " + i, r.shouldDrop(1));
        }
        feed(r, 1, 10 * MS, now);
        assertTrue(r.shouldDrop(1));
    }

    @Test public void residencyBaselineIsTheMinimumOfTheLastTenSecondsOnly() {
        DecoderQueuePolicy.Residency r = new DecoderQueuePolicy.Residency();
        long now = 1_000_000_000L;
        now = feed(r, 90, 4 * MS, now);
        long stepNs = 100 * MS;
        // A steady 10 ms from here on: the 4 ms floor ages out after ten seconds.
        long excessEarly = 0;
        for (int i = 0; i < 120; i++) {
            long excess = r.onOutput(10 * MS, now);
            if (i == 0) excessEarly = excess;
            now += stepNs;
        }
        assertEquals("still compared with the old floor", 6 * MS, excessEarly);
        assertEquals("the floor aged out", 0, r.onOutput(10 * MS, now));
    }

    @Test public void residencyResetForgetsTheBaselineAndTheStreak() {
        DecoderQueuePolicy.Residency r = new DecoderQueuePolicy.Residency();
        long now = feed(r, 20, 4 * MS, 1_000_000_000L);
        feed(r, 40, 10 * MS, now);
        assertTrue(r.shouldDrop(1));
        r.reset();
        assertFalse(r.shouldDrop(1));
        assertEquals(0, r.onOutput(10 * MS, 9_000_000_000L));
    }

    @Test public void inFlightCountsFramesUntilTheirVsyncPasses() {
        DecoderQueuePolicy.InFlight f = new DecoderQueuePolicy.InFlight();
        f.onReleased(1000, 100 * MS);
        assertEquals(1, f.count(90 * MS));
        assertFalse(f.full(90 * MS));
        f.onReleased(2000, 117 * MS);
        assertTrue("two released, none shown", f.full(90 * MS));
        assertEquals("the first one's vsync passed", 1, f.count(105 * MS));
        assertEquals(0, f.count(120 * MS));
    }

    @Test public void inFlightRenderedFramesAndOlderOnesNoLongerCount() {
        DecoderQueuePolicy.InFlight f = new DecoderQueuePolicy.InFlight();
        f.onReleased(1000, 500 * MS);
        f.onReleased(2000, 517 * MS);
        assertTrue(f.full(400 * MS));
        f.onRendered(1000);
        assertEquals(1, f.count(400 * MS));
        // A later frame being shown means the compositor dropped the earlier one.
        f.onRendered(2000);
        assertEquals(0, f.count(400 * MS));
        f.onReleased(3000, 534 * MS);
        assertEquals(1, f.count(400 * MS));
    }

    @Test public void inFlightKeepsTheNewestFramesWhenMoreAreReleasedThanItHasSlots() {
        DecoderQueuePolicy.InFlight f = new DecoderQueuePolicy.InFlight();
        for (int i = 1; i <= 20; i++) f.onReleased(i * 1000L, 1_000 * MS + i * MS);
        assertEquals(8, f.count(0));
        assertTrue(f.full(0));
        f.reset();
        assertEquals(0, f.count(0));
        f.onReleased(1, 10);
        assertEquals("a reset also forgets what was rendered", 1, f.count(0));
    }
}

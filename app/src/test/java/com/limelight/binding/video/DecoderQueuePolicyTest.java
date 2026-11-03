package com.limelight.binding.video;

import org.junit.Test;

import java.util.ArrayList;
import java.util.List;
import java.util.Random;

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

    // Debt: 60 fps with a 1 ms floor, as in the issue 7 captures from a Shield.
    private static final long FRAME_NS = 16_666_667L;

    /**
     * A decoder in Smoothest that keeps surplus frames waiting inside it, a frame interval each,
     * on top of a phase and the 1 ms floor; a drop takes one away (unless dropsHelp is off).
     * Frames arrive one per interval, so the frames behind an output are its input-to-output time
     * in whole intervals unless behind says otherwise. Outputs jitter by up to 1 ms, and every
     * tenth comes out early (5 ms, nothing behind it), as in the captures. When the policy asks,
     * a newer output has come out within the wait on one output in newerEvery (0: never); the
     * first capture had one within 2 ms of 5% of its debt outputs, the second of 30%.
     */
    private static final class QueuedDecoder {
        final DecoderQueuePolicy.Debt debt = new DecoderQueuePolicy.Debt();
        final List<String> events = new ArrayList<>();
        long nowNs = 1_000_000_000L;
        long frameNs = FRAME_NS;
        int surplus;
        long phaseNs;
        boolean dips = true;
        boolean dropsHelp = true;
        int newerEvery = 1;
        int behind = -1;
        int outputs;
        int drops;
        int waits;

        /** The first frames of a stream come straight out, before anything queues up. */
        void start() {
            for (int i = 0; i < 3; i++) output(MS);
        }

        void run(long durationNs) {
            long endNs = nowNs + durationNs;
            while (nowNs < endNs) {
                long jitterNs = ((outputs * 7919L) % 2001 - 1000) * 1000L;
                long i2oNs = Math.max(MS, MS + surplus * frameNs + phaseNs + jitterNs);
                output(dips && outputs % 10 == 9 ? 5 * MS : i2oNs);
            }
        }

        private void output(long i2oNs) {
            debt.onOutput(i2oNs, nowNs, frameNs);
            if (debt.shouldDrop(behind >= 0 ? behind : (int) (i2oNs / frameNs))) {
                if (newerEvery > 0 && outputs % newerEvery == 0) {
                    debt.onDropped();
                    drops++;
                    if (dropsHelp && surplus > 0) surplus--;
                } else {
                    debt.onNoNewer();
                    waits++;
                }
            }
            String event = debt.takeEvent();
            if (event != null) events.add(event);
            outputs++;
            nowNs += frameNs;
        }
    }

    @Test public void debtOfOneFrameIsDroppedOnceAfterTheWindow() {
        // The first 0.7.12 capture: 23-24 ms for a minute.
        QueuedDecoder d = new QueuedDecoder();
        d.start();
        d.surplus = 1;
        d.phaseNs = 6 * MS;
        d.run(1_000 * MS);
        assertEquals("not before the window", 0, d.drops);
        d.run(1_000 * MS);
        assertEquals(1, d.drops);
        assertEquals(0, d.surplus);
        d.run(60_000 * MS);
        assertEquals("the 6 ms phase left is under a frame", 1, d.drops);
        assertEquals(1, d.events.size());
        assertTrue(d.events.get(0), d.events.get(0).startsWith("Decoder debt recovered: 1 stale output dropped"));
    }

    @Test public void debtReplaysTheCaptureThatWentFromThirtySixToThirtyOneToSeventeen() {
        // The second 0.7.12 capture: 36 ms, then 31, then 17, draining only after 48 s.
        QueuedDecoder d = new QueuedDecoder();
        d.start();
        d.surplus = 2;
        d.phaseNs = 5 * MS / 3;
        d.run(1_000 * MS);
        assertEquals(0, d.drops);
        d.run(1_000 * MS);
        assertEquals("two frames, one at a time", 2, d.drops);
        assertEquals(0, d.surplus);
        d.run(23_000 * MS);
        assertEquals(2, d.drops);

        d.surplus = 1;
        d.phaseNs = 40 * MS / 3;
        d.run(2_000 * MS);
        assertEquals(3, d.drops);
        d.run(16_000 * MS);
        assertEquals("13 ms of phase is under a frame, with nothing behind it", 3, d.drops);

        d.surplus = 1;
        d.phaseNs = 0;
        d.run(2_000 * MS);
        assertEquals(4, d.drops);
        assertEquals(0, d.surplus);
        d.run(30_000 * MS);
        assertEquals(4, d.drops);
        for (String event : d.events) assertTrue(event, event.startsWith("Decoder debt "));
    }

    @Test public void debtSteadyOneRefreshFromTheFirstOutputIsTheFloor() {
        QueuedDecoder d = new QueuedDecoder();
        d.dips = false;
        d.surplus = 1;
        d.run(120_000 * MS);
        assertEquals(0, d.drops);
        assertTrue(d.events.isEmpty());
    }

    @Test public void debtJitteryHealthyStreamsAndOneOffStallsNeverDrop() {
        DecoderQueuePolicy.Debt debt = new DecoderQueuePolicy.Debt();
        Random random = new Random(7);
        long now = 1_000_000_000L;
        for (int i = 0; i < 60 * 120; i++) {
            long i2o = MS + random.nextInt(6_000) * 1000L;
            int behind = 0;
            if (random.nextInt(4) == 0) {
                // A quarter of the frames arrive in bursts and wait for the one ahead.
                i2o = 20 * MS + random.nextInt(10_000) * 1000L;
                behind = 1;
            }
            if (i >= 3000 && i < 3030) {
                // Half a second of stall.
                i2o = 35 * MS;
                behind = 2;
            }
            debt.onOutput(i2o, now, FRAME_NS);
            assertFalse("output " + i, debt.shouldDrop(behind));
            now += FRAME_NS + (random.nextInt(4_000) - 2_000) * 1000L;
        }
        assertNull(debt.takeEvent());
    }

    @Test public void debtSubFrameDriftIsLeftAlone() {
        // The 0.7.13 capture: the display runs 45 ppm slower than the host, so the wait for the
        // display grows by 0.045 ms a second with nothing behind it. Three minutes: 1 to 9 ms.
        DecoderQueuePolicy.Debt debt = new DecoderQueuePolicy.Debt();
        long now = 1_000_000_000L;
        for (int i = 0; i < 60 * 180; i++) {
            long i2o = MS + i * 750L + ((i * 7919L) % 1001 - 500) * 1000L;
            debt.onOutput(Math.max(MS, i2o), now, FRAME_NS);
            assertFalse("output " + i, debt.shouldDrop(i2o >= FRAME_NS ? 1 : 0));
            now += FRAME_NS;
        }
        assertNull(debt.takeEvent());
    }

    @Test public void debtDropThatDoesNotShortenTheQueueRaisesTheFloor() {
        QueuedDecoder d = new QueuedDecoder();
        d.dips = false;
        d.dropsHelp = false;
        d.start();
        d.surplus = 2;
        d.run(3_000 * MS);
        assertEquals(1, d.drops);
        assertTrue(d.events.get(0), d.events.get(0).startsWith("Decoder debt paused, a drop did not shorten the queue"));
        assertTrue(d.events.get(0), d.events.get(0).contains("floor raised to 3"));
        d.run(30_000 * MS);
        d.surplus = 0;
        d.run(2_000 * MS);
        d.surplus = 2;
        d.run(3_000 * MS);
        assertEquals("the same level again is the decoder's", 1, d.drops);
        d.dropsHelp = true;
        d.surplus = 3;
        d.run(3_000 * MS);
        assertEquals("a frame above the raised floor is still debt", 2, d.drops);
        assertTrue(d.events.get(1), d.events.get(1).startsWith("Decoder debt recovered: 1 stale output dropped"));
    }

    @Test public void debtDecoderPipeliningDeeperInMotionCostsOneDropAStream() {
        // 120 fps: 4 ms for a still scene, 10-11 ms in motion against a 6.25 ms threshold, with a
        // frame in the decoder behind each motion output. Ten scene changes cost one drop.
        QueuedDecoder d = new QueuedDecoder();
        d.frameNs = 8_333_333L;
        d.dips = false;
        d.dropsHelp = false;
        d.start();
        for (int i = 0; i < 10; i++) {
            d.phaseNs = 3 * MS;
            d.run(3_000 * MS);
            d.phaseNs = 19 * MS / 2;
            d.run(5_000 * MS);
        }
        assertEquals(1, d.drops);
        assertEquals(0, d.waits);
        assertEquals(1, d.events.size());
        assertTrue(d.events.get(0), d.events.get(0).startsWith("Decoder debt paused, a drop did not shorten the queue"));
        assertTrue(d.events.get(0), d.events.get(0).contains("floor raised"));

        d.dropsHelp = true;
        d.surplus = 1;
        d.run(3_000 * MS);
        assertEquals("debt on top of the pipelining is still caught", 2, d.drops);
        assertEquals(0, d.surplus);
        assertTrue(d.events.get(1), d.events.get(1).startsWith("Decoder debt recovered: 1 stale output dropped"));
    }

    @Test public void debtDropsOnlyForANewerOutputAndStopsWaitingForFramesThatNeverComeOut() {
        // After loss the decoder discards corrupt frames silently, so the input count says a frame
        // is behind until the next flush while none will come out.
        QueuedDecoder d = new QueuedDecoder();
        d.start();
        d.surplus = 1;
        d.phaseNs = 6 * MS;
        d.behind = 1;
        d.newerEvery = 0;
        d.run(3_000 * MS);
        assertEquals(0, d.drops);
        assertEquals(DecoderQueuePolicy.Debt.MAX_WAITS, d.waits);
        assertEquals(1, d.events.size());
        assertTrue(d.events.get(0), d.events.get(0).startsWith("Decoder debt paused, no newer output came out: 0 stale outputs dropped"));
        assertTrue(d.events.get(0), d.events.get(0).contains("floor raised"));
        d.run(60_000 * MS);
        d.surplus = 0;
        d.run(3_000 * MS);
        d.surplus = 1;
        d.run(5_000 * MS);
        assertEquals("no more waits at the level learned", DecoderQueuePolicy.Debt.MAX_WAITS, d.waits);

        d.newerEvery = 1;
        d.surplus = 2;
        d.run(3_000 * MS);
        assertEquals("deeper debt with a newer output is still dropped", 1, d.drops);
        assertEquals(1, d.surplus);
        assertEquals(DecoderQueuePolicy.Debt.MAX_WAITS, d.waits);

        // At 30 fps a window holds fewer outputs than the wait budget: the window ends it.
        QueuedDecoder slow = new QueuedDecoder();
        slow.frameNs = 33_333_333L;
        slow.start();
        slow.surplus = 1;
        slow.phaseNs = 6 * MS;
        slow.behind = 1;
        slow.newerEvery = 0;
        slow.run(5_000 * MS);
        assertEquals(0, slow.drops);
        assertTrue(slow.waits > 0 && slow.waits < DecoderQueuePolicy.Debt.MAX_WAITS);
        assertEquals(1, slow.events.size());
        assertTrue(slow.events.get(0), slow.events.get(0).startsWith("Decoder debt paused, no newer output came out"));
    }

    @Test public void debtReplaysBothCapturesWithFewNewerOutputs() {
        // Within 2 ms of a debt output a newer one came out for 5% of them in the first capture
        // and 30% in the second. One in 19 still recovers within a second of the window.
        QueuedDecoder first = new QueuedDecoder();
        first.newerEvery = 19;
        first.start();
        first.surplus = 1;
        first.phaseNs = 6 * MS;
        first.run(2_500 * MS);
        assertEquals(1, first.drops);
        assertEquals(0, first.surplus);
        assertTrue(first.waits < 19);
        assertTrue(first.events.get(0), first.events.get(0).startsWith("Decoder debt recovered: 1 stale output dropped"));

        QueuedDecoder second = new QueuedDecoder();
        second.newerEvery = 19;
        second.start();
        second.surplus = 2;
        second.phaseNs = 5 * MS / 3;
        second.run(2_500 * MS);
        assertEquals(2, second.drops);
        assertEquals(0, second.surplus);
        assertTrue(second.waits < 2 * 19);
        second.run(30_000 * MS);
        assertEquals(2, second.drops);
        assertEquals(1, second.events.size());
        assertTrue(second.events.get(0), second.events.get(0).startsWith("Decoder debt recovered: 2 stale outputs dropped"));
    }

    @Test public void debtStopsAtTheDropLimit() {
        QueuedDecoder d = new QueuedDecoder();
        d.dips = false;
        d.start();
        d.surplus = 20;
        d.run(5_000 * MS);
        assertEquals(DecoderQueuePolicy.Debt.MAX_DROPS, d.drops);
        assertEquals(1, d.events.size());
        assertTrue(d.events.get(0), d.events.get(0).startsWith("Decoder debt paused at the drop limit: 8 stale outputs dropped"));
    }

    @Test public void debtNeedsAFrameBehindAndResetEndsARecovery() {
        DecoderQueuePolicy.Debt debt = new DecoderQueuePolicy.Debt();
        long now = 1_000_000_000L;
        assertEquals("unknown times are skipped", 0, debt.onOutput(0, now, FRAME_NS));
        assertEquals(0, debt.onOutput(-5, now, FRAME_NS));
        assertEquals("the first output is the floor", 0, debt.onOutput(2 * MS, now, FRAME_NS));
        for (int i = 0; i < 100; i++) {
            now += FRAME_NS;
            assertEquals(22 * MS, debt.onOutput(24 * MS, now, FRAME_NS));
        }
        assertFalse("nothing behind it, nothing to drop for", debt.shouldDrop(0));
        assertTrue(debt.shouldDrop(1));
        debt.reset();
        now += FRAME_NS;
        assertEquals("a flush starts a new floor", 0, debt.onOutput(24 * MS, now, FRAME_NS));
        assertFalse(debt.shouldDrop(1));
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

    @Test public void decoderDepthCountsFromTheLastFlushAndIsNeverNegative() {
        assertEquals(3, DecoderQueuePolicy.depth(10, 7, 0));
        // A flush with three frames inside: they are gone, so the count starts again from zero.
        assertEquals(0, DecoderQueuePolicy.depth(10, 7, 3));
        assertEquals(2, DecoderQueuePolicy.depth(12, 7, 3));
        assertEquals("outputs returned for frames from before the flush", 0, DecoderQueuePolicy.depth(10, 9, 3));
    }
}

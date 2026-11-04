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
     * bufferNs is the wait for a free display buffer when the display's queue holds one too many,
     * with nothing behind the output: a drop with nothing behind gives the buffer back and clears
     * it (unless dropsHelp is off). Frames arrive one per interval, so the frames behind an output
     * are its input-to-output time in whole intervals unless behind says otherwise. Outputs jitter
     * by up to 1 ms, and every tenth comes out early (5 ms, nothing behind it), as in the captures.
     * Watching the display, inputs come with up to half a millisecond of jitter and outputs follow
     * them, except while bufferNs is positive: the output then leaves on the display's clock, and it
     * is the input's jitter that shows in input-to-output. A drop leaves bufferNs a frame lower, room
     * the display's queue fills again.
     * When the policy asks with a frame behind, a newer output has come out within the wait on one
     * output in newerEvery (0: never); the first capture had one within 2 ms of 5% of its debt
     * outputs, the second of 30%.
     */
    private static final class QueuedDecoder {
        final DecoderQueuePolicy.Debt debt = new DecoderQueuePolicy.Debt();
        final boolean watching;
        final List<String> events = new ArrayList<>();
        long nowNs = 1_000_000_000L;
        long frameNs = FRAME_NS;
        int surplus;
        long phaseNs;
        long bufferNs;
        boolean dips = true;
        boolean dropsHelp = true;
        int newerEvery = 1;
        int behind = -1;
        int outputs;
        int drops;
        int heldDrops;
        int waits;
        long firstDropNs;
        long peakI2oNs;

        QueuedDecoder() {
            this(false);
        }

        QueuedDecoder(boolean watching) {
            this.watching = watching;
            debt.watchDisplay(watching);
        }

        /** The first frames of a stream come straight out, before anything queues up. */
        void start() {
            for (int i = 0; i < 3; i++) output(MS);
        }

        void run(long durationNs) {
            long endNs = nowNs + durationNs;
            while (nowNs < endNs) {
                long jitterNs = ((outputs * 7919L) % 2001 - 1000) * 1000L;
                long i2oNs = Math.max(MS, MS + surplus * frameNs + phaseNs + Math.max(0, bufferNs) + jitterNs);
                output(dips && outputs % 10 == 9 ? 5 * MS : i2oNs);
            }
        }

        /** The display's queue fills by rateNs a second for durationNs. */
        void drift(long rateNs, long durationNs) {
            long endNs = nowNs + durationNs;
            while (nowNs < endNs) {
                long before = nowNs;
                run(frameNs);
                bufferNs += rateNs * (nowNs - before) / 1_000_000_000L;
            }
        }

        private void output(long i2oNs) {
            int framesBehind = behind >= 0 ? behind : (int) (i2oNs / frameNs);
            if (watching) {
                long arrivalNs = ((outputs * 104729L) % 1001 - 500) * 1000L;
                debt.onOutput(i2oNs, i2oNs, bufferNs > 0 ? nowNs : nowNs + arrivalNs + i2oNs, frameNs, framesBehind);
            } else {
                debt.onOutput(i2oNs, nowNs, frameNs);
            }
            peakI2oNs = Math.max(peakI2oNs, i2oNs);
            if (debt.shouldDrop()) {
                if (framesBehind == 0) {
                    debt.onDropped();
                    drops++;
                    heldDrops++;
                    if (dropsHelp) bufferNs = watching ? Math.min(0, bufferNs - frameNs) : 0;
                } else if (newerEvery > 0 && outputs % newerEvery == 0) {
                    debt.onDropped();
                    drops++;
                    if (dropsHelp && surplus > 0) surplus--;
                } else {
                    debt.onNoNewer();
                    waits++;
                }
                if (drops > 0 && firstDropNs == 0) firstDropNs = nowNs;
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

        // 31 ms: a frame in the decoder and 13 ms waiting for a display buffer, nothing behind.
        d.surplus = 1;
        d.phaseNs = 0;
        d.bufferNs = 40 * MS / 3;
        d.run(2_000 * MS);
        assertEquals("the frame, then the held output for the buffer", 4, d.drops);
        assertEquals(1, d.heldDrops);
        assertEquals(0, d.surplus);
        assertEquals(0, d.bufferNs);
        d.run(16_000 * MS);
        assertEquals(4, d.drops);

        d.surplus = 1;
        d.run(2_000 * MS);
        assertEquals(5, d.drops);
        assertEquals(0, d.surplus);
        d.run(30_000 * MS);
        assertEquals(5, d.drops);
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
            assertFalse("output " + i, debt.shouldDrop());
            now += FRAME_NS + (random.nextInt(4_000) - 2_000) * 1000L;
        }
        assertNull(debt.takeEvent());
    }

    @Test public void debtSubFrameDriftCostsOneDropEachHalfFrame() {
        // The 0.7.13 capture: the display runs 45 ppm slower than the host, so the wait for a
        // display buffer grows by 0.045 ms a second with nothing behind it. Half a frame takes
        // about three minutes; one dropped output then starts it over.
        QueuedDecoder d = new QueuedDecoder();
        d.dips = false;
        d.start();
        long startNs = d.nowNs;
        d.drift(45_000, 150_000 * MS);
        assertEquals("under half a frame for 150 s", 0, d.drops);
        d.drift(45_000, 90_000 * MS);
        assertEquals(1, d.drops);
        assertEquals(1, d.heldDrops);
        assertTrue("dropped at " + (d.firstDropNs - startNs) / 1e9 + " s",
                d.firstDropNs - startNs >= 180_000 * MS && d.firstDropNs - startNs <= 215_000 * MS);
        assertEquals(1, d.events.size());
        assertTrue(d.events.get(0), d.events.get(0).startsWith("Decoder debt recovered: 1 stale output dropped"));
    }

    @Test public void debtShieldPlateauWithNothingBehindIsDroppedWithinSeconds() {
        // The 4.4.0 capture (sm1): from 62 s the wait for a display buffer rose by about 0.4 ms a
        // second to 10-12 ms and stayed there, nothing behind each output, until a stall's catch-up
        // dropped a frame at 110 s and brought it back to 1 ms.
        QueuedDecoder d = new QueuedDecoder();
        d.start();
        d.run(60_000 * MS);
        assertEquals(0, d.drops);
        long riseNs = d.nowNs;
        d.drift(400_000, 25_000 * MS);
        d.run(20_000 * MS);
        assertEquals(1, d.drops);
        assertEquals(1, d.heldDrops);
        // The drop freed the buffer; what the last seconds of the rise added stays under the threshold.
        assertTrue("buffer " + d.bufferNs / 1e6 + " ms", d.bufferNs < 4 * MS);
        // Half a frame is reached 21 s into the rise, jitter and the window take a few more.
        assertTrue("dropped " + (d.firstDropNs - riseNs) / 1e9 + " s into the rise",
                d.firstDropNs - riseNs <= 27_000 * MS);
        d.run(60_000 * MS);
        assertEquals("healthy again", 1, d.drops);

        // A drop that does not free anything is the decoder's own: one drop, then the floor rises.
        QueuedDecoder own = new QueuedDecoder();
        own.dropsHelp = false;
        own.start();
        own.bufferNs = 11 * MS;
        own.run(60_000 * MS);
        assertEquals(1, own.drops);
        assertTrue(own.events.get(0), own.events.get(0).startsWith("Decoder debt paused, a drop did not shorten the queue"));
    }

    /** One recorded Shield stream (src/test/resources/pacing): output times, input-to-output, frames behind. */
    private static long[][] shieldCapture(String name) throws java.io.IOException {
        List<long[]> rows = new ArrayList<>();
        try (java.io.InputStream raw = DecoderQueuePolicyTest.class.getResourceAsStream("/pacing/shield-smoothest-" + name + ".txt.gz");
             java.io.BufferedReader in = new java.io.BufferedReader(new java.io.InputStreamReader(
                     new java.util.zip.GZIPInputStream(raw), java.nio.charset.StandardCharsets.US_ASCII))) {
            long nowNs = 1_000_000_000L;
            for (String line; (line = in.readLine()) != null; ) {
                if (line.startsWith("#")) continue;
                String[] f = line.split(" ");
                nowNs += Long.parseLong(f[0]) * 1000;
                rows.add(new long[] {nowNs, Long.parseLong(f[1]) * 1000, Long.parseLong(f[2])});
            }
        }
        return rows.toArray(new long[0][]);
    }

    /** Replays a capture; returns the time of the first output the policy gives way on (0: none). */
    private static long firstDebtDrop(long[][] capture, long fromNs, long toNs) {
        DecoderQueuePolicy.Debt debt = new DecoderQueuePolicy.Debt();
        long startNs = capture[0][0];
        for (long[] output : capture) {
            long t = output[0] - startNs;
            if (t < fromNs) continue;
            if (t >= toNs) break;
            debt.onOutput(output[1], output[0], FRAME_NS);
            if (debt.shouldDrop()) return t;
        }
        return 0;
    }

    @Test public void debtReplaysTheRecordedShieldStreams() throws Exception {
        long[][] sm1 = shieldCapture("sm1");
        long dropNs = firstDebtDrop(sm1, 0, Long.MAX_VALUE);
        // Input-to-output passed 6 ms at 65 s and half a frame (8.3 ms) at 69 s; three quarters
        // of a frame, the earlier threshold, it never held, so the 45 s plateau stayed.
        assertTrue("first drop at " + dropNs / 1e9 + " s", dropNs >= 65_000 * MS && dropNs <= 75_000 * MS);
        assertEquals("after the stall's catch-up at 110 s", 0, firstDebtDrop(sm1, 110_000 * MS, Long.MAX_VALUE));
        // The second capture: a healthy 1-2 ms for two minutes, with 4-6 ms of jitter at times.
        assertEquals(0, firstDebtDrop(shieldCapture("sm2"), 0, Long.MAX_VALUE));
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

    @Test public void debtResetEndsARecovery() {
        DecoderQueuePolicy.Debt debt = new DecoderQueuePolicy.Debt();
        long now = 1_000_000_000L;
        assertEquals("unknown times are skipped", 0, debt.onOutput(0, now, FRAME_NS));
        assertEquals(0, debt.onOutput(-5, now, FRAME_NS));
        assertEquals("the first output is the floor", 0, debt.onOutput(2 * MS, now, FRAME_NS));
        for (int i = 0; i < 100; i++) {
            now += FRAME_NS;
            assertEquals(10 * MS, debt.onOutput(12 * MS, now, FRAME_NS));
        }
        assertTrue("10 ms over the floor is past half a frame", debt.shouldDrop());
        debt.reset();
        now += FRAME_NS;
        assertEquals("a flush starts a new floor", 0, debt.onOutput(12 * MS, now, FRAME_NS));
        assertFalse(debt.shouldDrop());
    }

    private static DecoderQueuePolicy.Debt watchingDebt() {
        DecoderQueuePolicy.Debt debt = new DecoderQueuePolicy.Debt();
        debt.watchDisplay(true);
        return debt;
    }

    /** As [firstDebtDrop], watching the display. The captures only hold the stamp after the queue call. */
    private static long firstWatchingDrop(long[][] capture) {
        DecoderQueuePolicy.Debt debt = watchingDebt();
        for (long[] output : capture) {
            debt.onOutput(output[1], output[1], output[0], FRAME_NS, (int) output[2]);
            if (debt.shouldDrop()) return output[0] - capture[0][0];
        }
        return 0;
    }

    /**
     * Replays a capture and acts on the policy: a held output dropped with nothing behind it gives
     * the outputs a frame of room, so they follow their recorded inputs again. This loop takes the
     * frame of room off itself, so a replay shows when the policy drops, and how long the wait got by
     * then. It cannot show that a drop at that point frees a buffer: only the debt drops recorded at
     * half a frame (8.7 to 0.9 ms) speak for that. Returns the drop times, and the highest median of
     * a second before the first one.
     */
    private static List<Long> replayDrops(DecoderQueuePolicy.Debt debt, long[][] capture, long[] peakBeforeNs) {
        List<Long> drops = new ArrayList<>();
        List<Long> second = new ArrayList<>();
        long startNs = capture[0][0];
        long secondNs = startNs;
        long roomNs = 0;
        for (long[] output : capture) {
            long inputNs = output[0] - output[1];
            long i2oNs = roomNs > 0 ? Math.max(Math.min(output[1], 900 * 1000L), output[1] - roomNs) : output[1];
            debt.onOutput(i2oNs, i2oNs, inputNs + i2oNs, FRAME_NS, (int) output[2]);
            second.add(i2oNs);
            if (output[0] - secondNs >= 1_000 * MS) {
                java.util.Collections.sort(second);
                if (roomNs == 0) peakBeforeNs[0] = Math.max(peakBeforeNs[0], second.get(second.size() / 2));
                second.clear();
                secondNs = output[0];
            }
            if (debt.shouldDrop()) {
                debt.onDropped();
                drops.add(output[0] - startNs);
                roomNs += FRAME_NS;
            }
        }
        return drops;
    }

    @Test public void debtWatchesTheDisplayOnlyOnTheShieldsDecoders() {
        assertTrue(DecoderQueuePolicy.watchesDisplayQueue("OMX.Nvidia.h265.decode"));
        assertTrue(DecoderQueuePolicy.watchesDisplayQueue("omx.nvidia.h264.decode"));
        assertTrue(DecoderQueuePolicy.watchesDisplayQueue("c2.nvidia.hevc.decoder"));
        assertFalse(DecoderQueuePolicy.watchesDisplayQueue("c2.qti.hevc.decoder.low_latency"));
        assertFalse(DecoderQueuePolicy.watchesDisplayQueue("c2.mtk.hevc.decoder"));
        assertFalse(DecoderQueuePolicy.watchesDisplayQueue("OMX.MTK.VIDEO.DECODER.HEVC"));
        assertFalse(DecoderQueuePolicy.watchesDisplayQueue("c2.android.hevc.decoder"));
    }

    @Test public void debtElsewhereGivesItsOldAnswersOnTheTracesThatChangedOnAShield() throws Exception {
        // What the policy answered before the display rules, on the 4.6.1 captures: half a frame only.
        assertEquals(0, firstDebtDrop(shieldCapture("461-sm3-rise"), 0, Long.MAX_VALUE));
        assertEquals(0, firstDebtDrop(shieldCapture("461-sm4-rise"), 0, Long.MAX_VALUE));
        assertEquals(0, firstDebtDrop(shieldCapture("461-sm4-start"), 0, Long.MAX_VALUE));
        assertEquals(0, firstDebtDrop(shieldCapture("461-sm2-tail"), 0, Long.MAX_VALUE));
        assertEquals(0, firstDebtDrop(shieldCapture("461-sm1-edge"), 0, Long.MAX_VALUE));

        // And the start-up burst still raises the floor, whatever its level.
        DecoderQueuePolicy.Debt debt = new DecoderQueuePolicy.Debt();
        long[][] start = shieldCapture("461-sm1-start");
        int drops = 0;
        String event = null;
        for (int i = 0; i < start.length; i++) {
            debt.onOutput(start[i][1], start[i][0], FRAME_NS);
            if (debt.shouldDrop()) {
                if (start[i][2] == 0 || i % 3 == 0) {
                    debt.onDropped();
                    drops++;
                } else {
                    debt.onNoNewer();
                }
            }
            String taken = debt.takeEvent();
            if (taken != null) event = taken;
        }
        assertEquals(1, drops);
        assertEquals("Decoder debt paused, a drop did not shorten the queue: 1 stale output dropped, "
                + "excess 51.0 -> 57.1 ms over a 11.1 ms floor, floor raised to 68.2 ms", event);
    }

    @Test public void debtElsewhereLeavesAHeldOutputAloneBelowHalfAFrame() {
        // The same held stream the watching tests drop within seconds.
        QueuedDecoder d = new QueuedDecoder();
        d.dips = false;
        d.start();
        d.run(15_000 * MS);
        d.bufferNs = 4 * MS;
        d.run(120_000 * MS);
        assertEquals(0, d.drops);
    }

    @Test public void debtWatchingSubFrameDriftCostsOneDropForEachFrameItAdds() {
        // The 0.7.13 and 4.6.1 captures: the display runs 45 ppm slower than the host, so the wait
        // for a display buffer grows by 0.045 ms a second with nothing behind it. It is seen within
        // a minute instead of the three half a frame takes. A dropped output then gives this model's
        // queue a frame of room, 16.67 / 0.045 s of that drift.
        QueuedDecoder d = new QueuedDecoder(true);
        d.dips = false;
        d.start();
        long startNs = d.nowNs;
        d.drift(45_000, 60_000 * MS);
        assertEquals(1, d.drops);
        assertEquals(1, d.heldDrops);
        assertTrue("dropped at " + (d.firstDropNs - startNs) / 1e9 + " s", d.firstDropNs - startNs <= 60_000 * MS);
        d.drift(45_000, 240_000 * MS);
        assertEquals("the frame of room lasts", 1, d.drops);
        assertTrue(d.events.get(0), d.events.get(0).startsWith("Decoder debt recovered: 1 stale output dropped"));

        // Half an hour costs what the drift adds, 30 * 60 * 45 us of a 16.7 ms frame, and no more.
        d.drift(45_000, 1_500_000 * MS);
        assertEquals(5, d.drops);
        assertEquals(5, d.events.size());
        assertTrue("peak " + d.peakI2oNs / 1e6 + " ms", d.peakI2oNs < 4 * MS);
    }

    @Test public void debtWatchingSeesAPlateauWithNothingBehindWithinSeconds() {
        // The 4.4.0 capture, as debtShieldPlateauWithNothingBehindIsDroppedWithinSeconds.
        QueuedDecoder d = new QueuedDecoder(true);
        d.start();
        d.run(60_000 * MS);
        assertEquals(0, d.drops);
        long riseNs = d.nowNs;
        d.drift(400_000, 25_000 * MS);
        d.run(20_000 * MS);
        assertEquals(1, d.drops);
        assertEquals(1, d.heldDrops);
        assertTrue("buffer " + d.bufferNs / 1e6 + " ms", d.bufferNs < 4 * MS);
        // Half a frame would be reached 21 s into the rise; ten seconds of held outputs come first.
        assertTrue("dropped " + (d.firstDropNs - riseNs) / 1e9 + " s into the rise",
                d.firstDropNs - riseNs <= 15_000 * MS);
        d.run(60_000 * MS);
        assertEquals("healthy again", 1, d.drops);

        // A drop that frees nothing with half a frame on the clock: one drop, and the gate stays off.
        QueuedDecoder own = new QueuedDecoder(true);
        own.dropsHelp = false;
        own.start();
        own.run(15_000 * MS);
        own.bufferNs = 11 * MS;
        own.run(60_000 * MS);
        assertEquals(1, own.drops);
        assertTrue(own.events.get(0), own.events.get(0).startsWith("Decoder debt paused, a drop did not shorten the queue"));
        own.bufferNs = 0;
        own.run(15_000 * MS);
        own.bufferNs = 13 * MS;
        own.run(30_000 * MS);
        assertEquals("under half a frame over the raised floor", 1, own.drops);
    }

    @Test public void debtWatchingReplaysTheRisingStreamsOfNightjar461() throws Exception {
        // sm3 (from 100 s): the wait starts at about 115 s, input-to-output passes 2 ms at 125 s, and
        // half a frame was reached at 259 s at 8.7 ms, before the recorded debt drop.
        long[] peakNs = new long[1];
        DecoderQueuePolicy.Debt sm3 = watchingDebt();
        List<Long> drops = replayDrops(sm3, shieldCapture("461-sm3-rise"), peakNs);
        assertEquals("one drop", 1, drops.size());
        assertTrue("dropped at " + (100 + drops.get(0) / 1e9) + " s", drops.get(0) >= 25_000 * MS && drops.get(0) <= 42_000 * MS);
        assertTrue("before the drop " + peakNs[0] / 1e6 + " ms", peakNs[0] < 5 * MS);
        String event = sm3.takeEvent();
        assertTrue(event, event.startsWith("Decoder debt recovered: 1 stale output dropped"));

        // sm4 (from 190 s, the minute it measures against): the wait starts at about 225 s. Half a frame came at 369 s.
        peakNs[0] = 0;
        drops = replayDrops(watchingDebt(), shieldCapture("461-sm4-rise"), peakNs);
        assertEquals(1, drops.size());
        assertTrue("dropped at " + (190 + drops.get(0) / 1e9) + " s", drops.get(0) >= 45_000 * MS && drops.get(0) <= 70_000 * MS);
        assertTrue("before the drop " + peakNs[0] / 1e6 + " ms", peakNs[0] < 5 * MS);
    }

    @Test public void debtWatchingNeedsAFreeLevelToMeasureAWaitAgainst() throws Exception {
        // sm4's start: 11 ms from the second second on, 8.1 ms over the smallest time against a half
        // frame of 8.3, for 14 s until a stall of the host's cleared it. That was the level from the
        // start, and nothing in the recent past says it can be lower.
        assertEquals(0, firstWatchingDrop(shieldCapture("461-sm4-start")));
    }

    @Test public void debtWatchingLeavesTheSteadyPartsOfNightjar461Alone() throws Exception {
        // sm2's end: free outputs at 1.2 ms, until the wait starts 6.5 s before the run ends.
        assertEquals(0, firstWatchingDrop(shieldCapture("461-sm2-tail")));
        // sm1's end: 62 ms to the screen with outputs half following their input, 1.6-2.4 ms.
        assertEquals(0, firstWatchingDrop(shieldCapture("461-sm1-edge")));
        // The earlier second capture: a healthy 1-2 ms for two minutes, with 4-6 ms of jitter at times.
        assertEquals(0, firstWatchingDrop(shieldCapture("sm2")));
    }

    @Test public void debtWatchingKeepsTheFloorOfAStartUpBurst() throws Exception {
        // sm1's start: frames behind by up to 450 ms, and a drop in the middle of it did not shorten
        // anything. On 4.6.1 the floor rose to 81 ms: residency_excess_ms read 0 for the 4.5 minutes
        // that followed, however high input-to-output went.
        DecoderQueuePolicy.Debt debt = watchingDebt();
        long[][] start = shieldCapture("461-sm1-start");
        boolean dropped = false;
        for (long[] output : start) {
            debt.onOutput(output[1], output[1], output[0], FRAME_NS, (int) output[2]);
            if (debt.shouldDrop() && !dropped) {
                debt.onDropped();
                dropped = true;
            }
            String event = debt.takeEvent();
            if (event != null) assertFalse(event, event.contains("floor raised"));
        }
        assertTrue(dropped);
        long nowNs = start[start.length - 1][0] + FRAME_NS;
        assertTrue("a later 30 ms is seen as excess", debt.onOutput(30 * MS, 30 * MS, nowNs, FRAME_NS, 0) > 20 * MS);
    }

    /** Five seconds light and five deep for an hour, a frame behind each deep output; no drop frees anything. */
    private static int[] deepInMotion(boolean watching, long frameNs, long deepNs) {
        DecoderQueuePolicy.Debt debt = new DecoderQueuePolicy.Debt();
        debt.watchDisplay(watching);
        Random random = new Random(5);
        long sceneFrames = 5_000_000_000L / frameNs;
        long now = 1_000_000_000L;
        int drops = 0;
        int raised = 0;
        for (long i = 0; i < 3_600_000_000_000L / frameNs; i++) {
            boolean deep = (i / sceneFrames) % 2 == 1;
            long i2o = (deep ? deepNs : 2 * MS) + random.nextInt(500_000);
            now += frameNs;
            if (watching) debt.onOutput(i2o, i2o, now + i2o, frameNs, deep ? 2 : 0);
            else debt.onOutput(i2o, now + i2o, frameNs);
            String event = debt.takeEvent();
            if (event != null && event.contains("floor raised")) raised++;
            if (debt.shouldDrop()) {
                debt.onDropped();
                drops++;
            }
        }
        return new int[] {drops, raised};
    }

    @Test public void debtLearnsADeepDecodersLevelEvenPastThreeFrames() {
        // A decoder that pipelines 60 ms in motion: one pause could be a stall, the second at the same
        // level is the decoder's. Not a drop per scene change.
        assertArrayEquals(new int[] {2, 1}, deepInMotion(true, FRAME_NS, 60 * MS));
        assertArrayEquals(new int[] {2, 1}, deepInMotion(true, 8_333_333L, 30 * MS));
        assertArrayEquals("under three frames the first time", new int[] {1, 1}, deepInMotion(true, FRAME_NS, 40 * MS));
        assertArrayEquals(new int[] {1, 1}, deepInMotion(false, FRAME_NS, 60 * MS));
        assertArrayEquals(new int[] {1, 1}, deepInMotion(false, 8_333_333L, 30 * MS));
    }

    @Test public void debtWatchingFreeOutputsAboveAWaitLevelNeverDrop() {
        // A heavier scene every minute: 3 ms over the light one, the outputs still following their input.
        QueuedDecoder d = new QueuedDecoder(true);
        d.start();
        for (int i = 0; i < 10; i++) {
            d.phaseNs = 0;
            d.run(15_000 * MS);
            d.phaseNs = 3 * MS;
            d.run(45_000 * MS);
        }
        assertEquals(0, d.drops);
        assertTrue(d.events.isEmpty());
    }

    @Test public void debtWatchingOutputsOnADecoderClockCostOneDropAStream() {
        // A decoder that hands out on its own clock looks held, but a drop frees nothing. Half a
        // frame is never reached, so the check on the first drop is what ends it.
        QueuedDecoder d = new QueuedDecoder(true);
        d.dropsHelp = false;
        d.dips = false;
        d.start();
        d.run(15_000 * MS);
        d.bufferNs = 3 * MS;
        d.run(30_000 * MS);
        assertEquals(1, d.drops);
        assertEquals(2, d.events.size());
        assertTrue(d.events.get(1), d.events.get(1).startsWith("Decoder debt: outputs still follow the display"));
        d.bufferNs = 0;
        d.run(15_000 * MS);
        d.bufferNs = 3 * MS;
        d.run(30_000 * MS);
        assertEquals("the check stays failed", 1, d.drops);

        // A flush is a new stream state, and a real wait after it is still caught.
        d.debt.reset();
        d.dropsHelp = true;
        d.bufferNs = 0;
        d.run(15_000 * MS);
        d.bufferNs = 3 * MS;
        d.run(15_000 * MS);
        assertEquals(2, d.drops);
    }

    @Test public void debtWatchingFramesBehindRuleTheDisplayOut() {
        // Input-to-output that does not follow the input with a frame in the decoder behind each
        // output is a decoder finishing one frame when the next one comes in.
        QueuedDecoder d = new QueuedDecoder(true);
        d.dips = false;
        d.start();
        d.run(15_000 * MS);
        d.behind = 1;
        d.bufferNs = 4 * MS;
        d.run(60_000 * MS);
        assertEquals(0, d.drops);
        assertEquals(0, d.waits);
    }

    @Test public void debtWatchingHeldOutputWaitsForOneWithNothingBehindIt() {
        // The same stream twice: the first says when it drops, the second has frames behind by then.
        QueuedDecoder first = heldWithNothingBehind();
        long heldNs = first.nowNs;
        first.run(20_000 * MS);
        assertEquals(1, first.heldDrops);

        QueuedDecoder d = heldWithNothingBehind();
        d.run(first.firstDropNs - heldNs - 300 * MS);
        assertEquals("not before the window", 0, d.drops);
        d.behind = 1;
        d.newerEvery = 1;
        d.run(600 * MS);
        assertEquals("a frame behind this one: no wait, no drop", 0, d.drops);
        assertEquals(0, d.waits);
        d.behind = 0;
        d.run(4_000 * MS);
        assertEquals(1, d.heldDrops);
    }

    private static QueuedDecoder heldWithNothingBehind() {
        QueuedDecoder d = new QueuedDecoder(true);
        d.dips = false;
        d.start();
        d.behind = 0;
        d.run(15_000 * MS);
        d.bufferNs = 3 * MS;
        return d;
    }

    /** Outputs at a level: on the display's grid when held, else following an input with half a ms of jitter. */
    private static final class WatchedStream {
        final DecoderQueuePolicy.Debt debt = watchingDebt();
        final Random random = new Random(23);
        final List<Long> drops = new ArrayList<>();
        final List<String> events = new ArrayList<>();
        long gridNs = 1_000_000_000L;

        /** Until durationNs has passed or the policy drops; returns whether it dropped. */
        boolean run(long durationNs, long levelNs, boolean held) {
            long endNs = gridNs + durationNs;
            int before = drops.size();
            while (gridNs < endNs && drops.size() == before) {
                gridNs += FRAME_NS;
                long jitterNs = random.nextInt(1_000_000) - 500_000;
                long i2oNs = held ? levelNs - jitterNs : levelNs + random.nextInt(100_000);
                debt.onOutput(i2oNs, i2oNs, held ? gridNs : gridNs + jitterNs + i2oNs, FRAME_NS, 0);
                String event = debt.takeEvent();
                if (event != null) events.add(event);
                if (debt.shouldDrop()) {
                    debt.onDropped();
                    drops.add(gridNs);
                }
            }
            return drops.size() > before;
        }
    }

    @Test public void debtWatchingBacksOffWhenADropChangesNothing() {
        // Outputs that read held for a while, then free again at the same level: the drop did nothing.
        WatchedStream s = new WatchedStream();
        for (int i = 0; i < 60; i++) {
            s.run(12_000 * MS, MS, false);
            if (s.run(30_000 * MS, 4 * MS, true)) s.run(8_000 * MS, 4 * MS, false);
        }
        assertEquals("first, then ten minutes on, then twenty", 3, s.drops.size());
        assertTrue(s.drops.get(1) - s.drops.get(0) >= DecoderQueuePolicy.Debt.GATE_BACKOFF_NS);
        assertTrue(s.drops.get(2) - s.drops.get(1) >= 2 * DecoderQueuePolicy.Debt.GATE_BACKOFF_NS);
        assertEquals(3, s.events.stream().filter(e -> e.contains("the next one waits")).count());
    }

    @Test public void debtWatchingStopsAtItsLimitAndLeavesHalfAFrame() {
        // Every drop ends the wait, so nothing backs off, but a flush is what renews the limit.
        WatchedStream s = new WatchedStream();
        for (int i = 0; i < 2 * DecoderQueuePolicy.Debt.MAX_GATE_DROPS; i++) {
            s.run(12_000 * MS, MS, false);
            if (s.run(30_000 * MS, 4 * MS, true)) s.run(8_000 * MS, MS, false);
        }
        assertEquals(DecoderQueuePolicy.Debt.MAX_GATE_DROPS, s.drops.size());
        assertEquals(1, s.events.stream().filter(e -> e.contains("24 drops for the display")).count());
        assertTrue("half a frame still counts", s.run(5_000 * MS, 12 * MS, true));
        int atLimit = s.drops.size();
        s.debt.reset();
        s.run(12_000 * MS, MS, false);
        assertTrue(s.run(30_000 * MS, 4 * MS, true));
        assertEquals(atLimit + 1, s.drops.size());
    }

    @Test public void debtWatchingMeasuresTheWaitAgainstTheLastMinute() {
        // One light scene at the start of a stream, then the level it stays at. Nothing is waiting
        // any more than it did a minute ago, whatever the outputs read.
        WatchedStream s = new WatchedStream();
        s.run(5_000 * MS, MS, false);
        s.run(120_000 * MS, 4 * MS, false);
        s.run(60_000 * MS, 4 * MS, true);
        assertTrue(s.drops.isEmpty());
    }

    @Test public void debtWatchesTheTimeFromBeforeTheQueueCall() {
        // A free decoder on a steady link, whose queue call returns up to a millisecond late. That is
        // in input-to-output, and no output follows it: read from there, the outputs look held.
        DecoderQueuePolicy.Debt debt = watchingDebt();
        Random random = new Random(29);
        long gridNs = 1_000_000_000L;
        for (int i = 0; i < 60 * 600; i++) {
            gridNs += FRAME_NS;
            long submitNs = gridNs + (long) (random.nextGaussian() * 150_000);
            long levelNs = i % 3600 < 1200 ? 3 * MS : 5 * MS;
            long outputNs = submitNs + levelNs + (long) (random.nextGaussian() * 50_000);
            long queuedNs = submitNs + (long) (Math.abs(random.nextGaussian()) * 600_000);
            debt.onOutput(outputNs - queuedNs, outputNs - submitNs, outputNs, FRAME_NS, 0);
            assertFalse("output " + i, debt.shouldDrop());
        }
    }

    @Test public void recentLevelForgetsALightSceneAfterAMinute() {
        DecoderQueuePolicy.RecentLevel level = new DecoderQueuePolicy.RecentLevel();
        long now = 1_000_000_000L;
        level.add(MS, now);
        assertEquals("nothing before a second has ended", Long.MAX_VALUE, level.lowNs());
        for (int i = 0; i < 60 * 10; i++) {
            now += FRAME_NS;
            level.add(i < 60 * 5 ? MS : 4 * MS, now);
        }
        assertEquals(MS, level.lowNs());
        assertEquals(4 * MS, level.lastNs());
        for (int i = 0; i < 60 * 60; i++) {
            now += FRAME_NS;
            level.add(4 * MS, now);
        }
        assertEquals(4 * MS, level.lowNs());
        level.reset();
        assertEquals(Long.MAX_VALUE, level.lowNs());
    }

    @Test public void displayGateWantsASlopeItsInputsCanShow() {
        // A free decoder whose decode noise is five times its input spread reads low slopes now and
        // then: one standard error of 0.25 over ten seconds at 60 fps.
        DecoderQueuePolicy.DisplayGate gate = new DecoderQueuePolicy.DisplayGate();
        Random random = new Random(19);
        long gridNs = 1_000_000_000L;
        for (int i = 0; i < 3600 * 60; i++) {
            gridNs += FRAME_NS;
            long decodeNs = (long) (5 * MS + random.nextGaussian() * MS);
            long submitNs = gridNs + (long) (random.nextGaussian() * 200_000);
            gate.onOutput(decodeNs, submitNs + decodeNs, FRAME_NS, true, false);
            assertFalse("output " + i, gate.held());
        }
    }

    @Test public void displayGateSaysNothingAboutInputsSteadierThanItsResolution() {
        // Outputs on a grid, inputs within 50 us of one: nothing for an output to have followed.
        DecoderQueuePolicy.DisplayGate gate = new DecoderQueuePolicy.DisplayGate();
        Random random = new Random(17);
        long now = 1_000_000_000L;
        for (int i = 0; i < 60 * 60; i++) {
            now += FRAME_NS;
            gate.onOutput(4 * MS + random.nextInt(50_000), now, FRAME_NS, true, false);
            assertFalse("output " + i, gate.held());
        }
    }

    @Test public void displayGateCountsFramesBehindOverEveryOutput() {
        // Every fourth output comes after a burst with a frame behind it: only its steps are large.
        DecoderQueuePolicy.DisplayGate gate = new DecoderQueuePolicy.DisplayGate();
        Random random = new Random(13);
        long now = 1_000_000_000L;
        for (int i = 0; i < 60 * 30; i++) {
            now += FRAME_NS;
            boolean burst = i % 4 == 0;
            gate.onOutput(burst ? 25 * MS : 3 * MS + random.nextInt(1_000_000), now, FRAME_NS, true, burst);
            assertFalse("output " + i, gate.held());
        }
    }

    @Test public void displayGateReadsOutputsThatFollowTheirInputAsFree() {
        DecoderQueuePolicy.DisplayGate gate = new DecoderQueuePolicy.DisplayGate();
        Random random = new Random(11);
        long gridNs = 1_000_000_000L;
        for (int i = 0; i < 60 * 15; i++) {
            gridNs += FRAME_NS;
            long i2oNs = 3 * MS + random.nextInt(500_000);
            gate.onOutput(i2oNs, gridNs + random.nextInt(1_000_000) + i2oNs, FRAME_NS, true, false);
            assertFalse("output " + i, gate.held());
        }
        gate.reset();
        for (int i = 0; i < 60 * 15; i++) {
            gridNs += FRAME_NS;
            gate.onOutput(3 * MS + random.nextInt(1_000_000), gridNs, FRAME_NS, true, false);
        }
        assertTrue(gate.held());
        gate.clear();
        assertFalse(gate.held());
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

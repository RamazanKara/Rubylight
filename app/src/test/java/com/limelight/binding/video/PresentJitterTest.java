package com.limelight.binding.video;

import org.junit.Assume;
import org.junit.Test;

import java.lang.reflect.Method;

import static org.junit.Assert.*;

public class PresentJitterTest {
    private static final long VSYNC_60 = 16_666_667L;

    private static PresentJitter enabled() {
        PresentJitter jitter = new PresentJitter();
        jitter.setEnabled(true);
        return jitter;
    }

    /** The first present, which only starts the window. */
    private static long start(PresentJitter jitter) {
        jitter.record(1_000_000_000L);
        return 1_000_000_000L;
    }

    /** One present after fromNs per interval, each given in vsyncs of VSYNC_60; returns the last present. */
    private static long present(PresentJitter jitter, long fromNs, int... vsyncs) {
        long ns = fromNs;
        for (int units : vsyncs) {
            ns += units * VSYNC_60;
            jitter.record(ns);
        }
        return ns;
    }

    private static int[] repeat(int units, int count) {
        int[] out = new int[count];
        java.util.Arrays.fill(out, units);
        return out;
    }

    @Test public void steadyPacingIsOneVsyncWithoutJudder() {
        PresentJitter jitter = enabled();
        present(jitter, start(jitter), repeat(1, 120));
        PresentJitter.Snapshot window = jitter.snapshot(VSYNC_60);
        assertEquals(120, window.count);
        assertEquals(1, window.modeUnits);
        assertEquals(100f, window.percent(1), 0.001f);
        assertEquals(0f, window.judderPercent, 0.001f);
        assertEquals(0f, window.jitterMs, 0.001f);
        assertEquals(16.667f, window.averageMs, 0.001f);
        assertEquals(new String(new char[30]).replace('\0', '▁'), window.timeline());
    }

    @Test public void oneLongFrameIsJudderAndShowsInItsTimelineColumn() {
        PresentJitter jitter = enabled();
        int[] vsyncs = repeat(1, 40);
        vsyncs[21] = 3;
        present(jitter, start(jitter), vsyncs);
        PresentJitter.Snapshot window = jitter.snapshot(VSYNC_60);
        assertEquals(40, window.count);
        assertEquals(39, window.histogram[1]);
        assertEquals(1, window.histogram[3]);
        assertEquals(2.5f, window.judderPercent, 0.001f);
        // Mean 42/40 vsyncs; deviation 39 * 2/40 + 1 * 78/40 vsyncs, over 40 frames.
        double mean = 42.0 / 40 * VSYNC_60;
        double deviation = (39 * Math.abs(VSYNC_60 - mean) + Math.abs(3 * VSYNC_60 - mean)) / 40 / 1e6;
        assertEquals(deviation, window.jitterMs, 0.001);
        // Frame 21 falls in the sixth column of four.
        assertEquals("▁▁▁▁▁▃▁▁▁▁", window.timeline());
    }

    @Test public void theRingKeepsTheNewest240Intervals() {
        PresentJitter jitter = enabled();
        long last = present(jitter, start(jitter), repeat(2, 100));
        present(jitter, last, repeat(1, PresentJitter.CAPACITY));
        assertEquals(PresentJitter.CAPACITY, jitter.size());
        PresentJitter.Snapshot window = jitter.snapshot(VSYNC_60);
        assertEquals(PresentJitter.CAPACITY, window.count);
        assertEquals(PresentJitter.CAPACITY, window.histogram[1]);
        assertEquals(0, window.histogram[2]);
        assertEquals(PresentJitter.TIMELINE_COLUMNS, window.timeline().length());
    }

    @Test public void theOldestIntervalStartsTheTimelineAfterTheRingWraps() {
        PresentJitter jitter = enabled();
        long last = present(jitter, start(jitter), repeat(1, PresentJitter.CAPACITY - 2));
        // The ring wraps here: the first of these is the newest-but-two, the 8 lands last.
        present(jitter, last, 1, 1, 1, 1, 8);
        PresentJitter.Snapshot window = jitter.snapshot(VSYNC_60);
        assertEquals(8, window.units[PresentJitter.CAPACITY - 1]);
        assertEquals('█', window.timeline().charAt(PresentJitter.TIMELINE_COLUMNS - 1));
        assertEquals('▁', window.timeline().charAt(0));
    }

    @Test public void vsyncsAreCountedForTheRefreshTheOverlayReads() {
        PresentJitter jitter = enabled();
        present(jitter, start(jitter), repeat(1, 10));
        // The same 60 Hz cadence on a 120 Hz screen stays two vsyncs on screen.
        PresentJitter.Snapshot window = jitter.snapshot(VSYNC_60 / 2);
        assertEquals(2, window.modeUnits);
        assertEquals(100f, window.percent(2), 0.001f);
        assertEquals("▂▂▂", window.timeline());
    }

    @Test public void presentsWithinOneVsyncCountZeroAndLongOnesShareTheLastBucket() {
        PresentJitter jitter = enabled();
        long ns = start(jitter);
        for (int i = 0; i < 4; i++) jitter.record(ns += 4_000_000L); // a quarter vsync: 0
        jitter.record(ns + 10 * VSYNC_60);                            // ten vsyncs: the 8+ bucket
        PresentJitter.Snapshot window = jitter.snapshot(VSYNC_60);
        assertEquals(4, window.histogram[0]);
        assertEquals(1, window.histogram[PresentJitter.MAX_UNITS]);
        assertEquals(".█", window.timeline());
    }

    @Test public void stallsAndPausesAreLeftOut() {
        PresentJitter jitter = enabled();
        long ns = present(jitter, start(jitter), 1, 1);
        jitter.record(ns += 13 * VSYNC_60);       // over 12 vsyncs: kept, but not counted at 60 Hz
        jitter.record(ns += 2_000_000_000L);      // a pause: never recorded
        jitter.record(ns += VSYNC_60);
        assertEquals(4, jitter.size());
        PresentJitter.Snapshot window = jitter.snapshot(VSYNC_60);
        assertEquals(3, window.count);
        assertEquals(3, window.histogram[1]);
        // At 240 Hz the 13 vsyncs of 60 Hz are 52: still over 12, so still left out.
        assertEquals(3, jitter.snapshot(VSYNC_60 / 4).count);
    }

    @Test public void outOfOrderPresentsKeepTheNewest() {
        PresentJitter jitter = enabled();
        start(jitter);
        jitter.record(1_000_000_000L + 2 * VSYNC_60);
        jitter.record(1_000_000_000L + VSYNC_60);
        jitter.record(1_000_000_000L + 3 * VSYNC_60);
        PresentJitter.Snapshot window = jitter.snapshot(VSYNC_60);
        assertEquals(2, window.count);
        assertEquals(1, window.histogram[2]);
        assertEquals(1, window.histogram[1]);
    }

    @Test public void disabledRecordsNothingAndSwitchingStartsAnEmptyWindow() {
        PresentJitter jitter = new PresentJitter();
        assertFalse(jitter.isEnabled());
        present(jitter, start(jitter), repeat(1, 10));
        assertEquals(0, jitter.size());
        assertNull(jitter.snapshot(VSYNC_60));

        jitter.setEnabled(true);
        long last = present(jitter, start(jitter), repeat(1, 10));
        assertEquals(10, jitter.size());
        assertNull(jitter.snapshot(0));
        jitter.setEnabled(false);
        assertNull(jitter.snapshot(VSYNC_60));
        jitter.setEnabled(true);
        assertEquals(0, jitter.size());
        // No interval spans the time it was off: the first present only starts the window.
        jitter.record(last + 100 * VSYNC_60);
        assertEquals(0, jitter.size());
    }

    @Test public void recordingAllocatesNothingOnOrOff() throws Exception {
        // The JVM's per-thread allocation counter, through reflection: unit tests compile against
        // android.jar, which has no java.lang.management.
        Object threads;
        Method allocatedBytes;
        try {
            threads = Class.forName("java.lang.management.ManagementFactory").getMethod("getThreadMXBean").invoke(null);
            Class<?> bean = Class.forName("com.sun.management.ThreadMXBean");
            Assume.assumeTrue(bean.isInstance(threads));
            Assume.assumeTrue((Boolean) bean.getMethod("isThreadAllocatedMemoryEnabled").invoke(threads));
            allocatedBytes = bean.getMethod("getThreadAllocatedBytes", long.class);
        } catch (ClassNotFoundException e) {
            Assume.assumeNoException(e);
            return;
        }
        PresentJitter off = new PresentJitter();
        PresentJitter on = enabled();
        Long id = Thread.currentThread().getId();
        // Warm up so class loading, compilation and the reflective call are not counted.
        record(off, on, 20_000);
        allocatedBytes.invoke(threads, id);
        long before = (Long) allocatedBytes.invoke(threads, id);
        record(off, on, 200_000);
        long allocated = (Long) allocatedBytes.invoke(threads, id) - before;
        // The reading itself allocates a few bytes, never one object per present.
        assertTrue("allocated " + allocated + " bytes", allocated < 1024);
    }

    private static void record(PresentJitter off, PresentJitter on, int frames) {
        long ns = 1_000_000_000L;
        for (int i = 0; i < frames; i++) {
            ns += VSYNC_60 + (i % 7) * 100_000L;
            off.record(ns);
            on.record(ns);
        }
    }
}

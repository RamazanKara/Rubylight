package com.limelight.binding.video;

import org.junit.Test;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

public class PyroWaveEarlyRecordsTest {
    @Test
    public void onlyRecordFramedPyroWaveAsksForEarlyRecords() {
        assertTrue(PyroWaveDecoderRenderer.wantsEarlyRecords(true, true, true));
        // A host without record framing sends whole containers: nothing to hand over early.
        assertFalse(PyroWaveDecoderRenderer.wantsEarlyRecords(true, false, true));
        assertFalse(PyroWaveDecoderRenderer.wantsEarlyRecords(false, true, true));
        // debug.nightjar.pyro_early and the default decide allowed.
        assertFalse(PyroWaveDecoderRenderer.wantsEarlyRecords(true, true, false));
    }

    @Test
    public void performanceHintsSeeTheEarlyPushesWithTheirFrame() {
        PyroWaveDecoderRenderer.EarlyWork work = new PyroWaveDecoderRenderer.EarlyWork();
        // No early pushes: the submit alone, as before.
        assertEquals(900_000, work.take(900_000));
        // Some 25 pushes while the frame arrived count with its submit, once.
        for (int i = 0; i < 25; i++) work.add(40_000);
        assertEquals(300_000 + 1_000_000, work.take(300_000));
        assertEquals(300_000, work.take(300_000));
        // A frame replaced before its submit: its pushes go with the next frame reported.
        work.add(500_000);
        work.add(600_000);
        assertEquals(1_400_000, work.take(300_000));
    }

    @Test
    public void overlaySaysHowManyFramesCameEarlyAndCountsTheFallbacks() {
        assertEquals("Early record push: 100% of frames",
                PyroWaveDecoderRenderer.earlyRecordsLine(new float[] {100, 0}));
        assertEquals("Early record push: 97% of frames | 9 pushed whole",
                PyroWaveDecoderRenderer.earlyRecordsLine(new float[] {97.4f, 9}));
        // Asked for, but no frame came with its packets: loud, not a blank line.
        assertEquals("Early record push: no frame came early",
                PyroWaveDecoderRenderer.earlyRecordsLine(new float[] {-1, 0}));
    }
}

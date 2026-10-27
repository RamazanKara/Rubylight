package com.limelight.nvstream;

import org.junit.Test;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

public class PyroWaveBitrateFloorTest {
    @Test
    public void matchesTheAuthorsRegressionAtCommonSizes() {
        assertEquals(89000, PyroWaveBitrateFloor.minimumKbps(1280, 720, 60, false, false));
        assertEquals(133000, PyroWaveBitrateFloor.minimumKbps(1920, 1080, 60, false, false));
        assertEquals(149000, PyroWaveBitrateFloor.minimumKbps(2560, 1440, 60, false, false));
        assertEquals(167000, PyroWaveBitrateFloor.minimumKbps(3840, 2160, 60, false, false));
        assertEquals(151000, PyroWaveBitrateFloor.minimumKbps(1920, 1080, 60, true, false));
    }

    @Test
    public void scalesWithFrameRateAndHdr() {
        assertEquals(265000, PyroWaveBitrateFloor.minimumKbps(1920, 1080, 120, false, false));
        assertEquals(159000, PyroWaveBitrateFloor.minimumKbps(1920, 1080, 60, false, true));
    }

    @Test
    public void sizesOutsideTheRegressionUseTheNearestEnd() {
        assertEquals(PyroWaveBitrateFloor.minimumKbps(1280, 720, 60, false, false),
                PyroWaveBitrateFloor.minimumKbps(854, 480, 60, false, false));
        assertEquals(PyroWaveBitrateFloor.minimumKbps(3840, 2160, 60, false, false),
                PyroWaveBitrateFloor.minimumKbps(7680, 4320, 60, false, false));
        assertTrue(PyroWaveBitrateFloor.minimumKbps(1280, 720, 0, false, false) > 0);
    }
}

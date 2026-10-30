package com.limelight.nvstream;

import org.junit.Test;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

public class PyroWaveBitrateFloorTest {
    // The host's own figures (rust/core/src/pyrowave.rs), so Nightjar never starts PyroWave at a
    // rate the host calls too low.
    @Test
    public void theFloorIsTheHosts() {
        assertEquals(138240, PyroWaveBitrateFloor.minimumKbps(1280, 720, 60));
        assertEquals(186624, PyroWaveBitrateFloor.minimumKbps(1920, 1080, 60));
        assertEquals(663552, PyroWaveBitrateFloor.minimumKbps(2560, 1440, 120));
        assertEquals(746496, PyroWaveBitrateFloor.minimumKbps(3840, 2160, 60));
        assertEquals(93312, PyroWaveBitrateFloor.minimumKbps(1920, 1080, 30));
        assertEquals(276480, PyroWaveBitrateFloor.minimumKbps(1280, 720, 120));
        assertEquals(0, PyroWaveBitrateFloor.minimumKbps(1280, 720, 0));
    }

    @Test
    public void theDefaultIsTheCleanPictureTarget() {
        assertEquals(276480, PyroWaveBitrateFloor.defaultKbps(1280, 720, 60));
        assertEquals(398132, PyroWaveBitrateFloor.defaultKbps(1920, 1080, 60));
        assertEquals(1415578, PyroWaveBitrateFloor.defaultKbps(2560, 1440, 120));
        assertEquals(1592525, PyroWaveBitrateFloor.defaultKbps(3840, 2160, 60));
        assertEquals(2000000, PyroWaveBitrateFloor.defaultKbps(3840, 2160, 120));
        int[][] sizes = {{1280, 720, 60}, {1920, 1080, 60}, {2560, 1440, 120}, {3840, 2160, 60}};
        for (int[] s : sizes) {
            assertTrue(PyroWaveBitrateFloor.defaultKbps(s[0], s[1], s[2]) > PyroWaveBitrateFloor.minimumKbps(s[0], s[1], s[2]));
        }
    }

    @Test
    public void aStreamStartsAtTheDefaultOrAChosenRateNoLowerThanTheFloor() {
        // Automatic: the default, whatever the stored conventional default was.
        assertEquals(398132, PyroWaveBitrateFloor.startKbps(20000, true, 1920, 1080, 60));
        assertEquals(1415578, PyroWaveBitrateFloor.startKbps(20000, true, 2560, 1440, 120));
        assertEquals(1592525, PyroWaveBitrateFloor.startKbps(20000, true, 3840, 2160, 60));
        // Chosen below the floor: raised to it. Above it: untouched, even past the default.
        assertEquals(186624, PyroWaveBitrateFloor.startKbps(20000, false, 1920, 1080, 60));
        assertEquals(200000, PyroWaveBitrateFloor.startKbps(200000, false, 1920, 1080, 60));
        assertEquals(900000, PyroWaveBitrateFloor.startKbps(900000, false, 1920, 1080, 60));
        assertEquals(2000000, PyroWaveBitrateFloor.startKbps(20000, false, 3840, 2160, 240));
    }
}

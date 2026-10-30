package com.limelight.nvstream;

import org.junit.Test;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

public class PyroWaveBitrateFloorTest {
    // The host's own figures (rust/core/src/pyrowave.rs), so Nightjar never defaults PyroWave to a
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
        assertEquals(276480, PyroWaveBitrateFloor.defaultKbps(1280, 720, 60, 0));
        assertEquals(398132, PyroWaveBitrateFloor.defaultKbps(1920, 1080, 60, 0));
        assertEquals(796263, PyroWaveBitrateFloor.defaultKbps(1920, 1080, 120, 0));
        int[][] sizes = {{1280, 720, 60}, {1920, 1080, 60}, {2560, 1440, 120}, {3840, 2160, 60}};
        for (int[] s : sizes) {
            assertTrue(PyroWaveBitrateFloor.defaultKbps(s[0], s[1], s[2], 0) > PyroWaveBitrateFloor.minimumKbps(s[0], s[1], s[2]));
        }
    }

    // A default over gigabit broke a first PyroWave stream on ordinary Ethernet and Wi-Fi.
    @Test
    public void theDefaultFitsTheLinkButNeverDropsBelowTheFloor() {
        // Link unknown: no more than 900 Mbps, unless the floor is higher.
        assertEquals(900000, PyroWaveBitrateFloor.defaultKbps(2560, 1440, 120, 0));
        assertEquals(900000, PyroWaveBitrateFloor.defaultKbps(3840, 2160, 60, 0));
        assertEquals(1492992, PyroWaveBitrateFloor.defaultKbps(3840, 2160, 120, 0));
        // A known link: 90% of it, again never below the floor.
        assertEquals(1592525, PyroWaveBitrateFloor.defaultKbps(3840, 2160, 60, 2500));
        assertEquals(1415578, PyroWaveBitrateFloor.defaultKbps(2560, 1440, 120, 2500));
        assertEquals(900000, PyroWaveBitrateFloor.defaultKbps(3840, 2160, 60, 1000));
        assertEquals(270000, PyroWaveBitrateFloor.defaultKbps(1920, 1080, 60, 300));
        assertEquals(186624, PyroWaveBitrateFloor.defaultKbps(1920, 1080, 60, 100));
        assertEquals(2000000, PyroWaveBitrateFloor.defaultKbps(3840, 2160, 120, 10000));
        assertEquals(2000000, PyroWaveBitrateFloor.defaultKbps(3840, 2160, 120, Integer.MAX_VALUE));
        // 10G at 4K60 and 2.5G get the full clean target; 1G at 1440p120 about 900 Mbps; unknown 900.
        assertEquals(1592525, PyroWaveBitrateFloor.defaultKbps(3840, 2160, 60, 10000));
        assertEquals(900000, PyroWaveBitrateFloor.defaultKbps(2560, 1440, 120, 1000));
    }

    @Test
    public void aStreamStartsAtTheDefaultOrTheChosenRate() {
        // Automatic: the default, whatever the stored conventional default was.
        assertEquals(398132, PyroWaveBitrateFloor.startKbps(20000, true, 1920, 1080, 60, 0));
        assertEquals(900000, PyroWaveBitrateFloor.startKbps(20000, true, 2560, 1440, 120, 0));
        assertEquals(900000, PyroWaveBitrateFloor.startKbps(20000, true, 3840, 2160, 60, 0));
        assertEquals(1592525, PyroWaveBitrateFloor.startKbps(20000, true, 3840, 2160, 60, 2500));
        // Chosen: kept as it is, below the floor (the settings warn) or past the default or the link.
        assertEquals(20000, PyroWaveBitrateFloor.startKbps(20000, false, 1920, 1080, 60, 0));
        assertEquals(200000, PyroWaveBitrateFloor.startKbps(200000, false, 1920, 1080, 60, 0));
        assertEquals(900000, PyroWaveBitrateFloor.startKbps(900000, false, 1920, 1080, 60, 0));
        assertEquals(1500000, PyroWaveBitrateFloor.startKbps(1500000, false, 3840, 2160, 60, 1000));
        assertEquals(20000, PyroWaveBitrateFloor.startKbps(20000, false, 3840, 2160, 240, 0));
    }
}

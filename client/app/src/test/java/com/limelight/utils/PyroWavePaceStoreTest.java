package com.limelight.utils;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public class PyroWavePaceStoreTest {
    @Test
    public void calibrationRoundTrips() {
        PyroWavePaceStore.Calibration saved = PyroWavePaceStore.Calibration.deserialize(
                new PyroWavePaceStore.Calibration(2350, 2500, 10000, true).serialize());
        assertEquals(2350, saved.paceMbps);
        assertEquals(2500, saved.clientLinkMbps);
        assertEquals(10000, saved.hostLinkMbps);
        assertTrue(saved.qualified);
    }

    @Test
    public void malformedValuesAreIgnored() {
        assertNull(PyroWavePaceStore.Calibration.deserialize(null));
        assertNull(PyroWavePaceStore.Calibration.deserialize("2,1,1,1,1"));
        assertNull(PyroWavePaceStore.Calibration.deserialize("1,0,1,1,1"));
        assertNull(PyroWavePaceStore.Calibration.deserialize("1,x,1,1,1"));
        assertNull(PyroWavePaceStore.Calibration.deserialize("1,500000,1,1,1"));
    }

    @Test
    public void paceIsOnlySentOnThePathItWasMeasuredOn() {
        PyroWavePaceStore.Calibration wired = new PyroWavePaceStore.Calibration(2350, 2500, 10000, true);
        assertEquals(2350, PyroWavePaceStore.paceMbpsFor(wired, 2500));
        // Now on Wi-Fi (no wired speed) or on a different port.
        assertEquals(0, PyroWavePaceStore.paceMbpsFor(wired, 0));
        assertEquals(0, PyroWavePaceStore.paceMbpsFor(wired, 1000));
        assertEquals(0, PyroWavePaceStore.paceMbpsFor(null, 2500));
    }

    @Test
    public void pathLinkIsTheSlowerKnownEnd() {
        assertEquals(1000, PyroWavePaceCalibration.pathLinkMbps(1000, 2500));
        assertEquals(2500, PyroWavePaceCalibration.pathLinkMbps(0, 2500));
        assertEquals(2500, PyroWavePaceCalibration.pathLinkMbps(2500, 0));
        assertEquals(0, PyroWavePaceCalibration.pathLinkMbps(0, 0));
        assertEquals(9999, PyroWavePaceCalibration.pathLinkMbps(0, 25000));
    }
}

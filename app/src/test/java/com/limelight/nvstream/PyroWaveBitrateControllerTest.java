package com.limelight.nvstream;

import org.junit.Test;

import static org.junit.Assert.assertEquals;

public class PyroWaveBitrateControllerTest {
    @Test
    public void timeSpentInMenusDoesNotCountAsHealthyRecovery() {
        PyroWaveBitrateController controller = new PyroWaveBitrateController(200000, 100000, 60, 0);
        assertEquals(0, controller.sample(1000, true, false, 0, 0, 2));
        controller.suspend();
        assertEquals(0, controller.sample(30000, true, false, 0, 0, 2));
        assertEquals(102000, controller.sample(50000, true, false, 0, 0, 2));
    }

    @Test
    public void sustainedDecodeOverBudgetReducesWithoutPacketLossOrQueueing() {
        PyroWaveBitrateController c = new PyroWaveBitrateController(300000, 300000, 120, 0);
        assertEquals(0, c.sample(4000, true, false, 0, 0, 9));
        assertEquals(255000, c.sample(5000, true, false, 0, 0, 9));
        c.applied(255000, 255000, 5000);
        assertEquals(0, c.sample(6000, true, false, 0, 0, 9));
        assertEquals(0, c.sample(7000, true, false, 0, 0, 9));
        assertEquals(216750, c.sample(8000, true, false, 0, 0, 9));
    }

    @Test
    public void decodeBudgetFollowsFrameRateAndTransientSpikesDoNotReduce() {
        PyroWaveBitrateController c = new PyroWaveBitrateController(400000, 300000, 60, 0);
        assertEquals(0, c.sample(4000, true, false, 0, 0, 20));
        assertEquals(0, c.sample(5000, true, false, 0, 0, 9));
        assertEquals(0, c.sample(24000, true, false, 0, 0, 9));
        assertEquals(306000, c.sample(25000, true, false, 0, 0, 9));
    }

    @Test
    public void missingDecodeMeasurementsAndLittleHeadroomPreventRecovery() {
        PyroWaveBitrateController c = new PyroWaveBitrateController(400000, 300000, 120, 0);
        assertEquals(0, c.sample(1000, true, false, 0, 0, 5));
        assertEquals(0, c.sample(21000, true, false, 0, 0, -1));
        assertEquals(0, c.sample(22000, true, false, 0, 0, 5));
        assertEquals(0, c.sample(42000, true, false, 0, 0, 7));
        assertEquals(0, c.sample(43000, true, false, 0, 0, 5));
        assertEquals(306000, c.sample(63000, true, false, 0, 0, 5));
    }

    @Test
    public void risingRecordLossLowersBeforeSustainedLossThreshold() {
        PyroWaveBitrateController c = new PyroWaveBitrateController(400000, 400000, 60, 0);
        assertEquals(0, c.sample(1000, true, false, 0, 0, 0));
        assertEquals(0, c.sample(2000, true, false, 0.5f, 0, 0));
        assertEquals(340000, c.sample(3000, true, false, 1, 0, 0));
    }

    @Test
    public void risingQueueDelayAndSustainedDelayLowerBitrate() {
        PyroWaveBitrateController c = new PyroWaveBitrateController(300000, 300000, 120, 0);
        assertEquals(0, c.sample(1000, true, false, 0, 4, 0));
        assertEquals(0, c.sample(2000, true, false, 0, 8, 0));
        assertEquals(255000, c.sample(3000, true, false, 0, 12, 0));
        c.applied(255000, 255000, 3000);
        assertEquals(0, c.sample(4000, true, false, 0, 40, 0));
        assertEquals(0, c.sample(5000, true, false, 0, 40, 0));
        assertEquals(216750, c.sample(6000, true, false, 0, 40, 0));
    }

    @Test
    public void transientCongestionDoesNotLowerAndRecoveryIsSlow() {
        PyroWaveBitrateController c = new PyroWaveBitrateController(400000, 300000, 60, 0);
        assertEquals(0, c.sample(4000, true, true, 10, 100, 0));
        assertEquals(0, c.sample(5000, true, false, 0, 0, 0));
        assertEquals(0, c.sample(24000, true, false, 0, 0, 0));
        assertEquals(306000, c.sample(25000, true, false, 0, 0, 0));
        c.applied(306000, 306000, 25000);
        assertEquals(0, c.sample(26000, true, false, 0, 0, 0));
        assertEquals(0, c.sample(45000, true, false, 0, 0, 0));
        assertEquals(312120, c.sample(46000, true, false, 0, 0, 0));
    }

    @Test
    public void runtimeCapAndChosenCeilingAreNeverExceeded() {
        PyroWaveBitrateController c = new PyroWaveBitrateController(3000000, 3000000, 60, 0);
        assertEquals(0, c.sample(1000, true, false, 0, 0, 0));
        assertEquals(0, c.sample(21000, true, false, 0, 0, 0));
        assertEquals(0, c.sample(22000, true, false, 5, 0, 0));
        assertEquals(1700000, c.sample(23000, true, false, 5, 0, 0));
        c = new PyroWaveBitrateController(300000, 299000, 60, 0);
        assertEquals(0, c.sample(1000, true, false, 0, 0, 0));
        assertEquals(300000, c.sample(21000, true, false, 0, 0, 0));
    }

    @Test
    public void hostAppliedCapAlsoLowersFloor() {
        PyroWaveBitrateController c = new PyroWaveBitrateController(400000, 400000, 60, 0);
        c.applied(400000, 5000, 1000);
        assertEquals(0, c.sample(2000, true, false, 0, 0, 0));
        assertEquals(0, c.sample(22000, true, false, 0, 0, 0));
        assertEquals(0, c.sample(23000, true, true, 10, 0, 0));
        assertEquals(0, c.sample(24000, true, true, 10, 0, 0));
    }

    @Test
    public void missingVideoAndBorderlineSamplesResetRecovery() {
        PyroWaveBitrateController c = new PyroWaveBitrateController(400000, 300000, 60, 0);
        assertEquals(0, c.sample(1000, true, false, 0, 0, 0));
        assertEquals(0, c.sample(20000, false, false, 0, 0, 0));
        assertEquals(0, c.sample(21000, true, false, 0, 0, 0));
        assertEquals(0, c.sample(40000, true, false, 0.2f, 0, 0));
        assertEquals(0, c.sample(41000, true, false, 0, 0, 0));
        assertEquals(0, c.sample(60000, true, false, 0, 10, 0));
        assertEquals(0, c.sample(61000, true, false, 0, 0, 0));
        assertEquals(306000, c.sample(81000, true, false, 0, 0, 0));
    }

    @Test
    public void nativeCongestionCanReduceWhenVideoStops() {
        PyroWaveBitrateController c = new PyroWaveBitrateController(400000, 400000, 60, 0);
        assertEquals(0, c.sample(5000, false, true, 0, 0, 0));
        assertEquals(340000, c.sample(6000, false, true, 0, 0, 0));
        c.applied(340000, 340000, 6000);
        assertEquals(0, c.sample(30000, false, false, 0, 0, 0));
    }

    @Test
    public void floorDoesNotRaiseSmallInitialBitrate() {
        PyroWaveBitrateController c = new PyroWaveBitrateController(1000, 1000, 60, 0);
        assertEquals(0, c.sample(5000, true, false, 5, 100, 0));
        assertEquals(0, c.sample(6000, true, false, 5, 100, 0));
    }

    @Test
    public void hotPhoneStepsDownSlowlyAndNotBelowHalf() {
        PyroWaveBitrateController controller = new PyroWaveBitrateController(20000, 20000, 60, 0);
        controller.setThermalLevel(2);
        assertEquals(0, controller.sample(5000, true, false, 0, 1, 2));
        assertEquals(0, controller.sample(6000, true, false, 0, 1, 2));
        assertEquals(17000, controller.sample(15000, true, false, 0, 1, 2));
        controller.applied(10000, 10000, 15000);
        assertEquals(0, controller.sample(31000, true, false, 0, 1, 2));
        assertEquals(0, controller.sample(32000, true, false, 0, 1, 2));
    }

    @Test
    public void warmPhoneHoldsTheRate() {
        PyroWaveBitrateController controller = new PyroWaveBitrateController(30000, 20000, 60, 0);
        controller.setThermalLevel(1);
        for (long now = 1000; now <= 60000; now += 1000) {
            assertEquals(0, controller.sample(now, true, false, 0, 1, 2));
        }
        controller.setThermalLevel(0);
        assertEquals(0, controller.sample(61000, true, false, 0, 1, 2));
    }
}

package com.limelight.binding.video;

import android.os.PowerManager;

import org.junit.Test;

import static org.junit.Assert.assertEquals;

public class ThermalMonitorTest {
    @Test
    public void headroomForecastLeadsThermalStatus() {
        assertEquals(ThermalMonitor.NORMAL, ThermalMonitor.levelFor(0.5f, PowerManager.THERMAL_STATUS_NONE));
        assertEquals(ThermalMonitor.WARM, ThermalMonitor.levelFor(0.8f, PowerManager.THERMAL_STATUS_LIGHT));
        assertEquals(ThermalMonitor.HOT, ThermalMonitor.levelFor(0.95f, PowerManager.THERMAL_STATUS_NONE));
    }

    @Test
    public void statusAloneWorksWithoutHeadroom() {
        assertEquals(ThermalMonitor.NORMAL, ThermalMonitor.levelFor(Float.NaN, PowerManager.THERMAL_STATUS_LIGHT));
        assertEquals(ThermalMonitor.WARM, ThermalMonitor.levelFor(Float.NaN, PowerManager.THERMAL_STATUS_MODERATE));
        assertEquals(ThermalMonitor.HOT, ThermalMonitor.levelFor(-1, PowerManager.THERMAL_STATUS_CRITICAL));
    }
}

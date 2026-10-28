package com.limelight.binding.video;

import com.limelight.R;

import org.junit.Test;

import static org.junit.Assert.assertEquals;

public class PyroWaveHdrFailureTest {
    @Test
    public void noTenBitFormatOnAndroid14SaysItNeedsAndroid15() {
        assertEquals(R.string.pyrowave_hdr_needs_android_15, PyroWaveDecoderRenderer.surfaceFailureSummary(1, 33));
        assertEquals(R.string.pyrowave_hdr_needs_android_15, PyroWaveDecoderRenderer.surfaceFailureSummary(1, 34));
    }

    @Test
    public void refusedWindowDataspaceBlamesTheDisplayOrDriver() {
        assertEquals(R.string.pyrowave_hdr_display_refused, PyroWaveDecoderRenderer.surfaceFailureSummary(2, 33));
        assertEquals(R.string.pyrowave_hdr_display_refused, PyroWaveDecoderRenderer.surfaceFailureSummary(2, 35));
    }

    @Test
    public void refusedHlgDataspaceNamesHlg() {
        assertEquals(R.string.pyrowave_hlg_display_refused, PyroWaveDecoderRenderer.surfaceFailureSummary(3, 33));
        assertEquals(R.string.pyrowave_hlg_display_refused, PyroWaveDecoderRenderer.surfaceFailureSummary(3, 35));
        // The window took HLG, then no longer held it after the first frame.
        assertEquals(R.string.pyrowave_hlg_display_refused, PyroWaveDecoderRenderer.surfaceFailureSummary(4, 33));
    }

    @Test
    public void otherFailuresKeepTheGenericVulkanMessage() {
        assertEquals(R.string.pyrowave_missing_swapchain, PyroWaveDecoderRenderer.surfaceFailureSummary(0, 33));
        assertEquals(R.string.pyrowave_missing_swapchain, PyroWaveDecoderRenderer.surfaceFailureSummary(1, 35));
    }
}

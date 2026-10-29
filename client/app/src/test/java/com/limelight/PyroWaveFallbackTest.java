package com.limelight;

import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.nvstream.jni.MoonBridge;

import org.junit.Test;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

public class PyroWaveFallbackTest {
    @Test
    public void pyroWaveChosenAndAnotherCodecStreamingIsAFallback() {
        // The TV's T6 run: PyroWave forced, and the stream came up as Dolby Vision on AV1 with no word.
        assertTrue(PyroWaveFallback.fellBack(true, MoonBridge.VIDEO_FORMAT_AV1_MAIN10));
        assertTrue(PyroWaveFallback.fellBack(true, MoonBridge.VIDEO_FORMAT_H265));
        assertTrue(PyroWaveFallback.fellBack(true, MoonBridge.VIDEO_FORMAT_H264));
    }

    @Test
    public void pyroWaveStreamingOrNotChosenIsNoFallback() {
        assertFalse(PyroWaveFallback.fellBack(true, MoonBridge.VIDEO_FORMAT_PYROWAVE));
        assertFalse(PyroWaveFallback.fellBack(true, MoonBridge.VIDEO_FORMAT_PYROWAVE_MAIN10_444));
        assertFalse(PyroWaveFallback.fellBack(false, MoonBridge.VIDEO_FORMAT_H265));
        // A stream without video (input only) has nothing to say.
        assertFalse(PyroWaveFallback.fellBack(true, 0));
    }

    @Test
    public void thePcIsBlamedOnlyWhenItsCodecsLackPyroWave() {
        assertEquals(PyroWaveFallback.Notice.PC,
                PyroWaveFallback.notice(MediaCodecDecoderRenderer.PYROWAVE_NOT_OFFERED, false));
        // The device's HDR surface failed, or every surface: the device's reason, never "This PC".
        assertEquals(PyroWaveFallback.Notice.DEVICE,
                PyroWaveFallback.notice(R.string.pyrowave_missing_swapchain, false));
        assertEquals(PyroWaveFallback.Notice.DEVICE, PyroWaveFallback.notice(R.string.pyrowave_needs_64_bit, false));
        // No known reason: no notice that would guess one.
        assertEquals(PyroWaveFallback.Notice.NONE, PyroWaveFallback.notice(0, false));
    }

    @Test
    public void anHdrLossTheConnectionAnnouncedIsNotSaidTwice() {
        assertEquals(PyroWaveFallback.Notice.NONE,
                PyroWaveFallback.notice(R.string.pyrowave_hdr_display_refused, true));
    }

    @Test
    public void theNoticeNamesTheCodecThatStreams() {
        assertEquals("AV1", PyroWaveFallback.codecName(MoonBridge.VIDEO_FORMAT_AV1_MAIN10));
        assertEquals("HEVC", PyroWaveFallback.codecName(MoonBridge.VIDEO_FORMAT_H265_MAIN10));
        assertEquals("H.264", PyroWaveFallback.codecName(MoonBridge.VIDEO_FORMAT_H264));
    }
}

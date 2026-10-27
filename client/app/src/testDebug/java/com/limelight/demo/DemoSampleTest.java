package com.limelight.demo;

import org.junit.Test;

import static org.junit.Assert.*;

public class DemoSampleTest {
    @Test
    public void aHardwareAv1DecoderPlaysTheAv1Sample() {
        assertEquals("av1", DemoSample.choose(true, true, true, true, false, false));
        assertEquals("av1", DemoSample.choose(true, false, true, true, false, false));
    }

    @Test
    public void withoutHardwareAv1AHardwareHevcDecoderPlaysTheHevcSample() {
        assertEquals("hevc", DemoSample.choose(false, true, true, true, false, false));
    }

    @Test
    public void softwareOnlyDecodersFallBackToTheEightBitSample() {
        // The TV emulator: AV1 and HEVC exist only as software decoders, so neither counts as hardware.
        assertEquals("h264", DemoSample.choose(false, false, true, true, false, false));
    }

    @Test
    public void aMissingAssetIsNeverChosen() {
        assertEquals("hevc", DemoSample.choose(true, true, false, true, false, false));
        assertEquals("h264", DemoSample.choose(true, true, false, false, false, false));
    }

    @Test
    public void theLaunchFlagAndAFailedPlaybackForceTheEightBitSample() {
        assertEquals("h264", DemoSample.choose(true, true, true, true, true, false));
        assertEquals("h264", DemoSample.choose(true, true, true, true, false, true));
    }

    @Test
    public void theOverlayDescribesTheSampleThatPlays() {
        assertEquals("AV1 · HDR", DemoSample.label("av1"));
        assertEquals("HEVC · HDR", DemoSample.label("hevc"));
        assertEquals("H.264 · SDR", DemoSample.label("h264"));
        assertEquals("H.264 · 8-bit SDR", DemoSample.detail("h264"));
        assertEquals("AV1 · 10-bit HDR", DemoSample.detail("av1"));
        assertEquals("H.264", DemoSample.decoderName("h264"));
    }
}

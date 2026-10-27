package com.limelight.binding.video;

import static org.junit.Assert.*;

import android.media.MediaFormat;
import android.view.Display;

import org.junit.Test;

/** Made-up capabilities: the bits are rubylight_protocol::hdr's, the types Display.HdrCapabilities'. */
public class HdrFormatPolicyTest {
    private static final int HDR10 = Display.HdrCapabilities.HDR_TYPE_HDR10;
    private static final int HLG = Display.HdrCapabilities.HDR_TYPE_HLG;
    private static final int DOLBY = Display.HdrCapabilities.HDR_TYPE_DOLBY_VISION;
    private static final int[] TV = {DOLBY, HDR10, HLG};

    @Test
    public void aDolbyVisionScreenAndDecoderAskForProfile81First() {
        assertEquals(HdrFormatPolicy.DV_81, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, TV, true, false));
        assertEquals(HdrFormatPolicy.DV_84,
                HdrFormatPolicy.pick(HdrFormatPolicy.DV_84 | HdrFormatPolicy.PQ, TV, true, false));
    }

    @Test
    public void dolbyVisionNeedsTheScreenTheDecoderAndTheHost() {
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, TV, false, false));
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, new int[] {HDR10, HLG}, true, false));
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.PQ | HdrFormatPolicy.HLG, TV, true, false));
        assertEquals(0, HdrFormatPolicy.pick(0, TV, true, false));
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, null, true, false));
    }

    @Test
    public void hlgStandsInOnlyForAScreenWithoutHdr10() {
        assertEquals(HdrFormatPolicy.HLG, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, new int[] {HLG}, false, false));
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, new int[] {HDR10, HLG}, false, false));
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.PQ, new int[] {HLG}, false, false));
        assertTrue(HdrFormatPolicy.hlgInstead(HdrFormatPolicy.HLG, new int[] {HLG}));
        assertFalse(HdrFormatPolicy.hlgInstead(HdrFormatPolicy.DV_81, new int[] {HLG}));
        assertFalse(HdrFormatPolicy.hlgInstead(HdrFormatPolicy.HLG, new int[] {HDR10}));
    }

    @Test
    public void aSurfaceTheClientDrawsItselfGetsStockHdr10() {
        // Packed full chroma, PyroWave and the Vulkan renderer draw BT.2020/PQ only.
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, TV, true, true));
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, new int[] {HLG}, false, true));
    }

    @Test
    public void hlgAndProfile84DecodeWithTheHlgTransfer() {
        assertEquals(MediaFormat.COLOR_TRANSFER_HLG, HdrFormatPolicy.transfer(HdrFormatPolicy.HLG));
        assertEquals(MediaFormat.COLOR_TRANSFER_HLG, HdrFormatPolicy.transfer(HdrFormatPolicy.DV_84));
        assertEquals(MediaFormat.COLOR_TRANSFER_ST2084, HdrFormatPolicy.transfer(HdrFormatPolicy.DV_81));
        assertEquals(MediaFormat.COLOR_TRANSFER_ST2084, HdrFormatPolicy.transfer(0));
        assertTrue(HdrFormatPolicy.dolbyVision(HdrFormatPolicy.DV_81));
        assertTrue(HdrFormatPolicy.dolbyVision(HdrFormatPolicy.DV_84));
        assertFalse(HdrFormatPolicy.dolbyVision(HdrFormatPolicy.HLG));
        assertFalse(HdrFormatPolicy.dolbyVision(0));
    }

    @Test
    public void theOverlaySaysWhatTheScreenGetsAndALostDolbyVisionDecoder() {
        assertEquals("HDR", HdrFormatPolicy.overlayWord(0, false));
        assertEquals("HLG", HdrFormatPolicy.overlayWord(HdrFormatPolicy.HLG, false));
        assertEquals("Dolby Vision 8.1", HdrFormatPolicy.overlayWord(HdrFormatPolicy.DV_81, true));
        assertEquals("Dolby Vision 8.4", HdrFormatPolicy.overlayWord(HdrFormatPolicy.DV_84, true));
        assertEquals("HDR10 (Dolby Vision decoder failed)", HdrFormatPolicy.overlayWord(HdrFormatPolicy.DV_81, false));
        assertEquals("HLG (Dolby Vision decoder failed)", HdrFormatPolicy.overlayWord(HdrFormatPolicy.DV_84, false));
        assertEquals("HDR10", HdrFormatPolicy.name(0));
    }
}

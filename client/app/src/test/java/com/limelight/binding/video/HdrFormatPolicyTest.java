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
    private static final int HDR10_PLUS = Display.HdrCapabilities.HDR_TYPE_HDR10_PLUS;
    private static final int[] TV = {DOLBY, HDR10, HLG};
    private static final HdrFormatPolicy.DrawnSurface DIRECT = HdrFormatPolicy.DrawnSurface.NONE;
    private static final HdrFormatPolicy.DrawnSurface DRAWN_PQ = HdrFormatPolicy.DrawnSurface.PQ_ONLY;
    private static final HdrFormatPolicy.DrawnSurface VULKAN = HdrFormatPolicy.DrawnSurface.PQ_HLG;
    // Android 14 (U) and 13 (T) for PyroWave HLG.
    private static final int U = 34;
    private static final int T = 33;

    @Test
    public void aDolbyVisionScreenAndDecoderAskForProfile81First() {
        assertEquals(HdrFormatPolicy.DV_81, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, TV, true, DIRECT));
        assertEquals(HdrFormatPolicy.DV_84,
                HdrFormatPolicy.pick(HdrFormatPolicy.DV_84 | HdrFormatPolicy.PQ, TV, true, DIRECT));
    }

    @Test
    public void dolbyVisionNeedsTheScreenTheDecoderAndTheHost() {
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, TV, false, DIRECT));
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, new int[] {HDR10, HLG}, true, DIRECT));
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.PQ | HdrFormatPolicy.HLG, TV, true, DIRECT));
        assertEquals(0, HdrFormatPolicy.pick(0, TV, true, DIRECT));
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, null, true, DIRECT));
    }

    @Test
    public void hlgStandsInOnlyForAScreenWithoutHdr10() {
        assertEquals(HdrFormatPolicy.HLG, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, new int[] {HLG}, false, DIRECT));
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, new int[] {HDR10, HLG}, false, DIRECT));
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.PQ, new int[] {HLG}, false, DIRECT));
        assertTrue(HdrFormatPolicy.hlgInstead(HdrFormatPolicy.HLG, new int[] {HLG}));
        assertFalse(HdrFormatPolicy.hlgInstead(HdrFormatPolicy.DV_81, new int[] {HLG}));
        assertFalse(HdrFormatPolicy.hlgInstead(HdrFormatPolicy.HLG, new int[] {HDR10}));
    }

    @Test
    public void aSurfaceTheClientDrawsItselfGetsStockHdr10() {
        // Packed full chroma and PyroWave draw BT.2020/PQ only.
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, TV, true, DRAWN_PQ));
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, new int[] {HLG}, false, DRAWN_PQ));
    }

    @Test
    public void theVulkanRendererAsksForHlgOnAnHlgOnlyScreenAndNeverDolbyVision() {
        assertEquals(HdrFormatPolicy.HLG, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, new int[] {HLG}, false, VULKAN));
        assertEquals(HdrFormatPolicy.HLG, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, new int[] {DOLBY, HLG}, true, VULKAN));
        // A screen with HDR10 streams HDR10, as with the decoder's own output.
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, new int[] {HDR10, HLG}, false, VULKAN));
        // The Dolby Vision TV with its decoder gets stock HDR10 on the Vulkan renderer.
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, TV, true, VULKAN));
        // No drawn surface ever gets a bit beyond PQ and HLG, whatever the host offers.
        int[][] screens = {null, {}, {HLG}, {HDR10}, {DOLBY}, {DOLBY, HLG}, TV, {DOLBY, HDR10, HLG, HDR10_PLUS}};
        for (int host = 0; host < 256; host++) {
            for (int[] screen : screens) {
                for (boolean decoder : new boolean[] {false, true}) {
                    for (HdrFormatPolicy.DrawnSurface surface : new HdrFormatPolicy.DrawnSurface[] {DRAWN_PQ, VULKAN}) {
                        int format = HdrFormatPolicy.pick(host, screen, decoder, surface);
                        assertEquals(0, format & ~(HdrFormatPolicy.PQ | HdrFormatPolicy.HLG));
                        assertFalse(HdrFormatPolicy.dolbyVision(format));
                    }
                }
            }
        }
    }

    @Test
    public void theVulkanRendererNamesDolbyVisionAndHdr10PlusItLeavesOut() {
        // The TV, its Dolby Vision decoder and a host offering Dolby Vision: Dolby Vision first.
        int[] tv = {DOLBY, HDR10, HLG, HDR10_PLUS};
        assertEquals(HdrFormatPolicy.DrawnLimit.DOLBY_VISION, HdrFormatPolicy.vulkanLimit(HdrFormatPolicy.ALL, tv, true, 0));
        // No decoder, or a host without Dolby Vision: HDR10+ for a PQ stream.
        assertEquals(HdrFormatPolicy.DrawnLimit.HDR10_PLUS, HdrFormatPolicy.vulkanLimit(HdrFormatPolicy.ALL, tv, false, 0));
        assertEquals(HdrFormatPolicy.DrawnLimit.HDR10_PLUS,
                HdrFormatPolicy.vulkanLimit(HdrFormatPolicy.PQ | HdrFormatPolicy.HLG, tv, true, 0));
        // HLG carries no HDR10+, and a screen without either loses nothing.
        assertEquals(HdrFormatPolicy.DrawnLimit.NONE,
                HdrFormatPolicy.vulkanLimit(HdrFormatPolicy.ALL, new int[] {HLG, HDR10_PLUS}, false, HdrFormatPolicy.HLG));
        assertEquals(HdrFormatPolicy.DrawnLimit.NONE, HdrFormatPolicy.vulkanLimit(HdrFormatPolicy.ALL, new int[] {HDR10, HLG}, true, 0));
        assertEquals(HdrFormatPolicy.DrawnLimit.NONE, HdrFormatPolicy.vulkanLimit(HdrFormatPolicy.ALL, null, true, 0));
        // A host that lists no formats (every host but Rubylight 3.0) sends neither.
        assertEquals(HdrFormatPolicy.DrawnLimit.NONE, HdrFormatPolicy.vulkanLimit(0, tv, true, 0));
    }

    @Test
    public void anHlgOnlyScreenThatGetsStockHdr10IsNeitherQuietNorReportedAsHdr() {
        // The screen got HDR because the PC offers HLG, then the stream stays PQ: a surface the
        // client draws itself, or a host whose DESCRIBE leaves HLG out (format 0 either way).
        int[] hlgOnly = {HLG};
        assertTrue(HdrPolicy.displayRefusal(33, true, hlgOnly, null, HdrFormatPolicy.ALL) == HdrPolicy.Refusal.NONE);
        for (int format : new int[] {HdrFormatPolicy.pick(HdrFormatPolicy.ALL, hlgOnly, false, DRAWN_PQ),
                HdrFormatPolicy.pick(HdrFormatPolicy.PQ, hlgOnly, false, DIRECT)}) {
            assertEquals(0, format);
            int transfer = HdrFormatPolicy.transfer(format);
            assertEquals(HdrPolicy.Status.PQ_ON_HLG_SCREEN, HdrPolicy.status(true, true, true,
                    HdrPolicy.Metadata.USABLE, null, transfer, HdrPolicy.hlgOnly(hlgOnly)));
            assertFalse(HdrPolicy.reportsHdrOutput(hlgOnly, transfer));
        }
        // The HLG stream it was asked for is quiet and reported as HDR.
        int transfer = HdrFormatPolicy.transfer(HdrFormatPolicy.pick(HdrFormatPolicy.ALL, hlgOnly, false, DIRECT));
        assertEquals(MediaFormat.COLOR_TRANSFER_HLG, transfer);
        assertEquals(HdrPolicy.Status.QUIET, HdrPolicy.status(true, true, true, HdrPolicy.Metadata.ABSENT,
                null, transfer, HdrPolicy.hlgOnly(hlgOnly)));
        assertTrue(HdrPolicy.reportsHdrOutput(hlgOnly, transfer));
    }

    @Test
    public void pyroWaveDrawsHlgOnlyForAHostThatOffersItThere() {
        int flagged = HdrFormatPolicy.ALL | HdrFormatPolicy.PYROWAVE_HLG;
        assertEquals(HdrFormatPolicy.PYROWAVE_HLG & HdrFormatPolicy.ALL, 0);
        assertEquals(VULKAN, HdrFormatPolicy.pyroWaveSurface(flagged, U));
        assertEquals(VULKAN, HdrFormatPolicy.pyroWaveSurface(HdrFormatPolicy.HLG | HdrFormatPolicy.PYROWAVE_HLG, U));
        // A 3.0 rc host refuses HLG on PyroWave; the flag without HLG offers nothing.
        assertEquals(DRAWN_PQ, HdrFormatPolicy.pyroWaveSurface(HdrFormatPolicy.ALL, U));
        assertEquals(DRAWN_PQ, HdrFormatPolicy.pyroWaveSurface(HdrFormatPolicy.PQ | HdrFormatPolicy.PYROWAVE_HLG, U));
        assertEquals(DRAWN_PQ, HdrFormatPolicy.pyroWaveSurface(0, U));

        int[] hlgOnly = {HLG};
        int surfaced = HdrFormatPolicy.pick(flagged, hlgOnly, true, HdrFormatPolicy.pyroWaveSurface(flagged, U));
        assertEquals(HdrFormatPolicy.HLG, surfaced);
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, hlgOnly, true,
                HdrFormatPolicy.pyroWaveSurface(HdrFormatPolicy.ALL, U)));
        // Never Dolby Vision, never the flag itself, and stock HDR10 on an HDR10 screen.
        for (int[] types : new int[][] {TV, {DOLBY, HLG}, {DOLBY}, hlgOnly, {HDR10, HDR10_PLUS}}) {
            int format = HdrFormatPolicy.pick(flagged, types, true, HdrFormatPolicy.pyroWaveSurface(flagged, U));
            assertEquals(0, format & ~(HdrFormatPolicy.PQ | HdrFormatPolicy.HLG));
        }
        assertEquals(0, HdrFormatPolicy.pick(flagged, TV, true, HdrFormatPolicy.pyroWaveSurface(flagged, U)));
    }

    @Test
    public void pyroWaveNamesDolbyVisionAndHdr10PlusItLeavesOut() {
        int flagged = HdrFormatPolicy.ALL | HdrFormatPolicy.PYROWAVE_HLG;
        assertEquals(HdrFormatPolicy.DrawnLimit.DOLBY_VISION, HdrFormatPolicy.pyroWaveLimit(flagged, TV, true, 0));
        // With an HLG stream too: the Dolby Vision words then say HLG (Game).
        assertEquals(HdrFormatPolicy.DrawnLimit.DOLBY_VISION,
                HdrFormatPolicy.pyroWaveLimit(flagged, new int[] {DOLBY, HLG}, true, HdrFormatPolicy.HLG));
        // Without the device's Dolby Vision decoder HEVC would show HDR10 too: nothing to say.
        assertEquals(HdrFormatPolicy.DrawnLimit.NONE, HdrFormatPolicy.pyroWaveLimit(flagged, TV, false, 0));
        assertEquals(HdrFormatPolicy.DrawnLimit.HDR10_PLUS,
                HdrFormatPolicy.pyroWaveLimit(flagged, new int[] {HDR10, HDR10_PLUS}, false, 0));
        // HLG carries no HDR10+, and a host without 3.0 HDR formats writes none.
        assertEquals(HdrFormatPolicy.DrawnLimit.NONE,
                HdrFormatPolicy.pyroWaveLimit(flagged, new int[] {HLG, HDR10_PLUS}, false, HdrFormatPolicy.HLG));
        assertEquals(HdrFormatPolicy.DrawnLimit.NONE,
                HdrFormatPolicy.pyroWaveLimit(0, new int[] {HDR10, HDR10_PLUS}, true, 0));
        assertEquals(HdrFormatPolicy.DrawnLimit.NONE,
                HdrFormatPolicy.pyroWaveLimit(flagged, new int[] {HDR10, HLG}, true, 0));
    }

    @Test
    public void anHlgOnlyScreenOnPyroWaveNamesTheHostUpdateOnlyWhereItHelps() {
        int[] hlgOnly = {HLG};
        assertTrue(HdrFormatPolicy.pyroWaveHlgNeedsHostUpdate(HdrFormatPolicy.ALL, hlgOnly, U));
        assertFalse(HdrFormatPolicy.pyroWaveHlgNeedsHostUpdate(HdrFormatPolicy.ALL | HdrFormatPolicy.PYROWAVE_HLG,
                hlgOnly, U));
        // No HLG at all from this host, or a screen with HDR10: an update is not what is missing.
        assertFalse(HdrFormatPolicy.pyroWaveHlgNeedsHostUpdate(HdrFormatPolicy.PQ, hlgOnly, U));
        assertFalse(HdrFormatPolicy.pyroWaveHlgNeedsHostUpdate(HdrFormatPolicy.ALL, new int[] {HDR10, HLG}, U));
        assertFalse(HdrFormatPolicy.pyroWaveHlgNeedsHostUpdate(HdrFormatPolicy.ALL, null, U));
        // Android 13 refuses HLG on PyroWave whatever the host: no update helps there.
        assertFalse(HdrFormatPolicy.pyroWaveHlgNeedsHostUpdate(HdrFormatPolicy.ALL, hlgOnly, T));
    }

    @Test
    public void pyroWaveAsksNoHlgBelowAndroid14() {
        // Android 13's ANativeWindow_setBuffersDataSpace refuses BT2020_HLG (-EINVAL on the Retroid
        // Pocket 6), so an HLG stream there could never start: PQ instead, from any host.
        int flagged = HdrFormatPolicy.ALL | HdrFormatPolicy.PYROWAVE_HLG;
        assertEquals(DRAWN_PQ, HdrFormatPolicy.pyroWaveSurface(flagged, T));
        assertEquals(0, HdrFormatPolicy.pick(flagged, new int[] {HLG}, true,
                HdrFormatPolicy.pyroWaveSurface(flagged, T)));
        assertEquals(VULKAN, HdrFormatPolicy.pyroWaveSurface(flagged, HdrFormatPolicy.PYROWAVE_HLG_MIN_SDK));
        assertEquals(VULKAN, HdrFormatPolicy.pyroWaveSurface(flagged, 35));
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

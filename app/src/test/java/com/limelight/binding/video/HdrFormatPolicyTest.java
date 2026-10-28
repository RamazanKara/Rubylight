package com.limelight.binding.video;

import static org.junit.Assert.*;

import android.media.MediaFormat;
import android.view.Display;

import com.limelight.nvstream.jni.MoonBridge;

import org.junit.Test;

/** Made-up capabilities: the bits are rubylight_protocol::hdr's, the types Display.HdrCapabilities'. */
public class HdrFormatPolicyTest {
    private static final int HDR10 = Display.HdrCapabilities.HDR_TYPE_HDR10;
    private static final int HLG = Display.HdrCapabilities.HDR_TYPE_HLG;
    private static final int DOLBY = Display.HdrCapabilities.HDR_TYPE_DOLBY_VISION;
    private static final int HDR10_PLUS = Display.HdrCapabilities.HDR_TYPE_HDR10_PLUS;
    private static final int[] TV = {DOLBY, HDR10, HLG};
    /** The TCL C6K's formats: H.264, HEVC Main10 and AV1 Main10. */
    private static final int TV_FORMATS = MoonBridge.VIDEO_FORMAT_H264 | MoonBridge.VIDEO_FORMAT_H265 |
            MoonBridge.VIDEO_FORMAT_H265_MAIN10 | MoonBridge.VIDEO_FORMAT_AV1_MAIN8 | MoonBridge.VIDEO_FORMAT_AV1_MAIN10;
    /** Its Dolby Vision decoders: profile 8 (c2.mtk.dvhe.st.decoder) and 10 (c2.mtk.dvav1.10.decoder). */
    private static final int TV_DECODERS = HdrFormatPolicy.DOLBY_VISION;
    private static final int HEVC_ONLY = TV_FORMATS & ~MoonBridge.VIDEO_FORMAT_MASK_AV1;
    private static final int AV1_ONLY = TV_FORMATS & ~MoonBridge.VIDEO_FORMAT_MASK_H265;
    private static final HdrFormatPolicy.DrawnSurface DIRECT = HdrFormatPolicy.DrawnSurface.NONE;
    private static final HdrFormatPolicy.DrawnSurface DRAWN_PQ = HdrFormatPolicy.DrawnSurface.PQ_ONLY;
    private static final HdrFormatPolicy.DrawnSurface VULKAN = HdrFormatPolicy.DrawnSurface.PQ_HLG;

    @Test
    public void theBitsAreTheProtocolsAndEachDolbyVisionProfileHasOneCodec() {
        assertEquals(31, HdrFormatPolicy.ALL);
        assertEquals(16, HdrFormatPolicy.DV_101);
        assertEquals(HdrFormatPolicy.DOLBY_VISION, HdrFormatPolicy.DOLBY_VISION_HEVC | HdrFormatPolicy.DOLBY_VISION_AV1);
        assertEquals(0, HdrFormatPolicy.DOLBY_VISION_HEVC & HdrFormatPolicy.DOLBY_VISION_AV1);
    }

    @Test
    public void aDolbyVisionScreenAsksForProfile101ThenProfile81ThenProfile84() {
        // AV1 first, as the handshake takes it before HEVC.
        assertEquals(HdrFormatPolicy.DV_101, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, TV, TV_DECODERS, TV_FORMATS, DIRECT));
        assertEquals(HdrFormatPolicy.DV_101, HdrFormatPolicy.pick(HdrFormatPolicy.PQ | HdrFormatPolicy.DV_101, TV,
                HdrFormatPolicy.DOLBY_VISION_AV1, AV1_ONLY, DIRECT));
        assertEquals(HdrFormatPolicy.DV_81, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, TV,
                HdrFormatPolicy.DOLBY_VISION_HEVC, TV_FORMATS, DIRECT));
        assertEquals(HdrFormatPolicy.DV_84, HdrFormatPolicy.pick(HdrFormatPolicy.DV_84 | HdrFormatPolicy.PQ, TV,
                TV_DECODERS, TV_FORMATS, DIRECT));
    }

    @Test
    public void eachProfileNeedsItsDecoderItsCodecAndTheHost() {
        // A host without AV1 offers profile 8 alone.
        int hevcHost = HdrFormatPolicy.ALL & ~HdrFormatPolicy.DV_101;
        assertEquals(HdrFormatPolicy.DV_81, HdrFormatPolicy.pick(hevcHost, TV, TV_DECODERS, TV_FORMATS, DIRECT));
        // HEVC chosen by the user, or no AV1 Main10 to carry profile 10: profile 8.
        assertEquals(HdrFormatPolicy.DV_81, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, TV, TV_DECODERS, HEVC_ONLY, DIRECT));
        assertEquals(HdrFormatPolicy.DV_81, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, TV, TV_DECODERS,
                TV_FORMATS & ~MoonBridge.VIDEO_FORMAT_AV1_MAIN10, DIRECT));
        // A decoder for the other profile only, or no codec to carry the one offered: stock HDR10.
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.PQ | HdrFormatPolicy.DV_101, TV,
                HdrFormatPolicy.DOLBY_VISION_HEVC, TV_FORMATS, DIRECT));
        assertEquals(0, HdrFormatPolicy.pick(hevcHost, TV, HdrFormatPolicy.DOLBY_VISION_AV1, TV_FORMATS, DIRECT));
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.PQ | HdrFormatPolicy.DV_101, TV, TV_DECODERS, HEVC_ONLY, DIRECT));
        assertEquals(0, HdrFormatPolicy.pick(hevcHost, TV, TV_DECODERS, AV1_ONLY, DIRECT));
    }

    @Test
    public void dolbyVisionNeedsTheScreenTheDecoderAndTheHost() {
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, TV, 0, TV_FORMATS, DIRECT));
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, new int[] {HDR10, HLG}, TV_DECODERS, TV_FORMATS, DIRECT));
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.PQ | HdrFormatPolicy.HLG, TV, TV_DECODERS, TV_FORMATS, DIRECT));
        assertEquals(0, HdrFormatPolicy.pick(0, TV, TV_DECODERS, TV_FORMATS, DIRECT));
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, null, TV_DECODERS, TV_FORMATS, DIRECT));
    }

    @Test
    public void hlgStandsInOnlyForAScreenWithoutHdr10() {
        assertEquals(HdrFormatPolicy.HLG, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, new int[] {HLG}, 0, TV_FORMATS, DIRECT));
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, new int[] {HDR10, HLG}, 0, TV_FORMATS, DIRECT));
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.PQ, new int[] {HLG}, 0, TV_FORMATS, DIRECT));
        assertTrue(HdrFormatPolicy.hlgInstead(HdrFormatPolicy.HLG, new int[] {HLG}));
        assertFalse(HdrFormatPolicy.hlgInstead(HdrFormatPolicy.DV_81, new int[] {HLG}));
        assertFalse(HdrFormatPolicy.hlgInstead(HdrFormatPolicy.HLG, new int[] {HDR10}));
    }

    @Test
    public void aSurfaceTheClientDrawsItselfGetsStockHdr10() {
        // Packed full chroma and PyroWave draw BT.2020/PQ only.
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, TV, TV_DECODERS, TV_FORMATS, DRAWN_PQ));
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, new int[] {HLG}, 0, TV_FORMATS, DRAWN_PQ));
    }

    @Test
    public void theVulkanRendererAsksForHlgOnAnHlgOnlyScreenAndNeverDolbyVision() {
        assertEquals(HdrFormatPolicy.HLG, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, new int[] {HLG}, 0, TV_FORMATS, VULKAN));
        assertEquals(HdrFormatPolicy.HLG,
                HdrFormatPolicy.pick(HdrFormatPolicy.ALL, new int[] {DOLBY, HLG}, TV_DECODERS, TV_FORMATS, VULKAN));
        // A screen with HDR10 streams HDR10, as with the decoder's own output.
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, new int[] {HDR10, HLG}, 0, TV_FORMATS, VULKAN));
        // The Dolby Vision TV with its decoders gets stock HDR10 on the Vulkan renderer, on HEVC and AV1.
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, TV, TV_DECODERS, TV_FORMATS, VULKAN));
        assertEquals(0, HdrFormatPolicy.pick(HdrFormatPolicy.ALL, TV, TV_DECODERS, AV1_ONLY, VULKAN));
        // No drawn surface ever gets a bit beyond PQ and HLG, whatever the host offers.
        int[][] screens = {null, {}, {HLG}, {HDR10}, {DOLBY}, {DOLBY, HLG}, TV, {DOLBY, HDR10, HLG, HDR10_PLUS}};
        for (int host = 0; host < 256; host++) {
            for (int[] screen : screens) {
                for (int decoders : new int[] {0, HdrFormatPolicy.DOLBY_VISION_HEVC, HdrFormatPolicy.DOLBY_VISION_AV1,
                        TV_DECODERS}) {
                    for (HdrFormatPolicy.DrawnSurface surface : new HdrFormatPolicy.DrawnSurface[] {DRAWN_PQ, VULKAN}) {
                        int format = HdrFormatPolicy.pick(host, screen, decoders, TV_FORMATS, surface);
                        assertEquals(0, format & ~(HdrFormatPolicy.PQ | HdrFormatPolicy.HLG));
                        assertFalse(HdrFormatPolicy.dolbyVision(format));
                    }
                }
            }
        }
    }

    @Test
    public void theVulkanRendererNamesDolbyVisionItLeavesOut() {
        // The TV, its Dolby Vision decoders and a host offering Dolby Vision.
        int[] tv = {DOLBY, HDR10, HLG, HDR10_PLUS};
        assertEquals(HdrFormatPolicy.DrawnLimit.DOLBY_VISION,
                HdrFormatPolicy.vulkanLimit(HdrFormatPolicy.ALL, tv, TV_DECODERS, TV_FORMATS));
        // Profile 10.1 alone (AV1) counts as well.
        assertEquals(HdrFormatPolicy.DrawnLimit.DOLBY_VISION, HdrFormatPolicy.vulkanLimit(
                HdrFormatPolicy.PQ | HdrFormatPolicy.DV_101, tv, HdrFormatPolicy.DOLBY_VISION_AV1, AV1_ONLY));
        // No decoder, or a host without Dolby Vision: nothing, as HDR10+ goes to the window.
        assertEquals(HdrFormatPolicy.DrawnLimit.NONE, HdrFormatPolicy.vulkanLimit(HdrFormatPolicy.ALL, tv, 0, TV_FORMATS));
        assertEquals(HdrFormatPolicy.DrawnLimit.NONE,
                HdrFormatPolicy.vulkanLimit(HdrFormatPolicy.PQ | HdrFormatPolicy.HLG, tv, TV_DECODERS, TV_FORMATS));
        assertEquals(HdrFormatPolicy.DrawnLimit.NONE,
                HdrFormatPolicy.vulkanLimit(HdrFormatPolicy.ALL, new int[] {HDR10, HLG}, TV_DECODERS, TV_FORMATS));
        assertEquals(HdrFormatPolicy.DrawnLimit.NONE, HdrFormatPolicy.vulkanLimit(HdrFormatPolicy.ALL, null, TV_DECODERS, TV_FORMATS));
        // A host that lists no formats (every host but Rubylight 3.0) sends no Dolby Vision.
        assertEquals(HdrFormatPolicy.DrawnLimit.NONE, HdrFormatPolicy.vulkanLimit(0, tv, TV_DECODERS, TV_FORMATS));
    }

    @Test
    public void theVulkanRendererPassesHdr10PlusOnlyToAnHdr10PlusScreenWithAPqStream() {
        int pq = MediaFormat.COLOR_TRANSFER_ST2084;
        assertTrue(HdrFormatPolicy.passesHdr10Plus(new int[] {DOLBY, HDR10, HLG, HDR10_PLUS}, pq));
        assertTrue(HdrFormatPolicy.passesHdr10Plus(new int[] {HDR10_PLUS}, pq));
        // HLG streams carry no HDR10+, and a screen without it gets none.
        assertFalse(HdrFormatPolicy.passesHdr10Plus(new int[] {HDR10, HLG, HDR10_PLUS}, MediaFormat.COLOR_TRANSFER_HLG));
        assertFalse(HdrFormatPolicy.passesHdr10Plus(new int[] {DOLBY, HDR10, HLG}, pq));
        assertFalse(HdrFormatPolicy.passesHdr10Plus(null, pq));
        assertEquals(pq, HdrFormatPolicy.transfer(0));
        assertEquals(MediaFormat.COLOR_TRANSFER_HLG, HdrFormatPolicy.transfer(HdrFormatPolicy.HLG));
    }

    @Test
    public void anHlgOnlyScreenThatGetsStockHdr10IsNeitherQuietNorReportedAsHdr() {
        // The screen got HDR because the PC offers HLG, then the stream stays PQ: a surface the
        // client draws itself, or a host whose DESCRIBE leaves HLG out (format 0 either way).
        int[] hlgOnly = {HLG};
        assertTrue(HdrPolicy.displayRefusal(33, true, hlgOnly, null, HdrFormatPolicy.ALL) == HdrPolicy.Refusal.NONE);
        for (int format : new int[] {HdrFormatPolicy.pick(HdrFormatPolicy.ALL, hlgOnly, 0, TV_FORMATS, DRAWN_PQ),
                HdrFormatPolicy.pick(HdrFormatPolicy.PQ, hlgOnly, 0, TV_FORMATS, DIRECT)}) {
            assertEquals(0, format);
            int transfer = HdrFormatPolicy.transfer(format);
            assertEquals(HdrPolicy.Status.PQ_ON_HLG_SCREEN, HdrPolicy.status(true, true, true,
                    HdrPolicy.Metadata.USABLE, null, transfer, HdrPolicy.hlgOnly(hlgOnly)));
            assertFalse(HdrPolicy.reportsHdrOutput(hlgOnly, transfer));
        }
        // The HLG stream it was asked for is quiet and reported as HDR.
        int transfer = HdrFormatPolicy.transfer(HdrFormatPolicy.pick(HdrFormatPolicy.ALL, hlgOnly, 0, TV_FORMATS, DIRECT));
        assertEquals(MediaFormat.COLOR_TRANSFER_HLG, transfer);
        assertEquals(HdrPolicy.Status.QUIET, HdrPolicy.status(true, true, true, HdrPolicy.Metadata.ABSENT,
                null, transfer, HdrPolicy.hlgOnly(hlgOnly)));
        assertTrue(HdrPolicy.reportsHdrOutput(hlgOnly, transfer));
    }

    @Test
    public void hlgAndProfile84DecodeWithTheHlgTransfer() {
        assertEquals(MediaFormat.COLOR_TRANSFER_HLG, HdrFormatPolicy.transfer(HdrFormatPolicy.HLG));
        assertEquals(MediaFormat.COLOR_TRANSFER_HLG, HdrFormatPolicy.transfer(HdrFormatPolicy.DV_84));
        assertEquals(MediaFormat.COLOR_TRANSFER_ST2084, HdrFormatPolicy.transfer(HdrFormatPolicy.DV_81));
        assertEquals(MediaFormat.COLOR_TRANSFER_ST2084, HdrFormatPolicy.transfer(HdrFormatPolicy.DV_101));
        assertEquals(MediaFormat.COLOR_TRANSFER_ST2084, HdrFormatPolicy.transfer(0));
        assertTrue(HdrFormatPolicy.dolbyVision(HdrFormatPolicy.DV_81));
        assertTrue(HdrFormatPolicy.dolbyVision(HdrFormatPolicy.DV_84));
        assertTrue(HdrFormatPolicy.dolbyVision(HdrFormatPolicy.DV_101));
        assertFalse(HdrFormatPolicy.dolbyVision(HdrFormatPolicy.HLG));
        assertFalse(HdrFormatPolicy.dolbyVision(0));
    }

    @Test
    public void dolbyVisionOnTheAutomaticCodecKeepsTheCodecThatCarriesIt() {
        // The handshake would take AV1, and the ANNOUNCE then drops profile 8 for AV1 HDR10.
        assertEquals(HEVC_ONLY, HdrFormatPolicy.videoFormats(HdrFormatPolicy.DV_81, TV_FORMATS, true));
        assertEquals(HEVC_ONLY, HdrFormatPolicy.videoFormats(HdrFormatPolicy.DV_84, TV_FORMATS, true));
        // Profile 10 keeps AV1, and HEVC goes so that no codec is left that can't carry it.
        assertEquals(AV1_ONLY, HdrFormatPolicy.videoFormats(HdrFormatPolicy.DV_101, TV_FORMATS, true));
        // Any other format, a codec the user chose, or no Main10 of the codec to carry it: unchanged.
        for (int format : new int[] {0, HdrFormatPolicy.PQ, HdrFormatPolicy.HLG}) {
            assertEquals(TV_FORMATS, HdrFormatPolicy.videoFormats(format, TV_FORMATS, true));
        }
        assertEquals(TV_FORMATS, HdrFormatPolicy.videoFormats(HdrFormatPolicy.DV_81, TV_FORMATS, false));
        assertEquals(TV_FORMATS, HdrFormatPolicy.videoFormats(HdrFormatPolicy.DV_101, TV_FORMATS, false));
        int noHevc10 = TV_FORMATS & ~MoonBridge.VIDEO_FORMAT_H265_MAIN10;
        assertEquals(noHevc10, HdrFormatPolicy.videoFormats(HdrFormatPolicy.DV_81, noHevc10, true));
        int noAv110 = TV_FORMATS & ~MoonBridge.VIDEO_FORMAT_AV1_MAIN10;
        assertEquals(noAv110, HdrFormatPolicy.videoFormats(HdrFormatPolicy.DV_101, noAv110, true));
        assertEquals(HEVC_ONLY, HdrFormatPolicy.videoFormats(HdrFormatPolicy.DV_81, HEVC_ONLY, true));
    }

    @Test
    public void theCodecsThatCarryAFormatAreItsCarriers() {
        // Profile 8 is HEVC Main10 only, profile 10 AV1 Main10 only; HLG and HDR10 go on any 10-bit codec.
        assertEquals(MoonBridge.VIDEO_FORMAT_H265_MAIN10, HdrFormatPolicy.carriers(HdrFormatPolicy.DV_81));
        assertEquals(MoonBridge.VIDEO_FORMAT_H265_MAIN10, HdrFormatPolicy.carriers(HdrFormatPolicy.DV_84));
        assertEquals(MoonBridge.VIDEO_FORMAT_AV1_MAIN10, HdrFormatPolicy.carriers(HdrFormatPolicy.DV_101));
        assertEquals(0, HdrFormatPolicy.carriers(HdrFormatPolicy.HLG));
        assertEquals(0, HdrFormatPolicy.carriers(0));
        assertEquals("HEVC", HdrFormatPolicy.codecNames(HdrFormatPolicy.carriers(HdrFormatPolicy.DV_81)));
        assertEquals("AV1", HdrFormatPolicy.codecNames(HdrFormatPolicy.carriers(HdrFormatPolicy.DV_101)));
        assertEquals("AV1", HdrFormatPolicy.codecNames(MoonBridge.VIDEO_FORMAT_MASK_AV1));
        assertEquals("AV1 and HEVC", HdrFormatPolicy.codecNames(MoonBridge.VIDEO_FORMAT_AV1_MAIN10 |
                MoonBridge.VIDEO_FORMAT_H265));
    }

    @Test
    public void theOverlaySaysWhatTheScreenGetsAndALostDolbyVisionDecoder() {
        assertEquals("HDR", HdrFormatPolicy.overlayWord(0, false));
        // HDR10+ reaching the screen through the Vulkan renderer; never for HLG.
        assertEquals("HDR10+", HdrFormatPolicy.overlayWord(0, false, true));
        assertEquals("HDR", HdrFormatPolicy.overlayWord(0, false, false));
        assertEquals("HLG", HdrFormatPolicy.overlayWord(HdrFormatPolicy.HLG, false, true));
        assertEquals("HLG", HdrFormatPolicy.overlayWord(HdrFormatPolicy.HLG, false));
        assertEquals("Dolby Vision 8.1", HdrFormatPolicy.overlayWord(HdrFormatPolicy.DV_81, true));
        assertEquals("Dolby Vision 8.4", HdrFormatPolicy.overlayWord(HdrFormatPolicy.DV_84, true));
        assertEquals("Dolby Vision 10.1", HdrFormatPolicy.overlayWord(HdrFormatPolicy.DV_101, true));
        assertEquals("HDR10 (Dolby Vision decoder failed)", HdrFormatPolicy.overlayWord(HdrFormatPolicy.DV_81, false));
        assertEquals("HLG (Dolby Vision decoder failed)", HdrFormatPolicy.overlayWord(HdrFormatPolicy.DV_84, false));
        assertEquals("HDR10 (Dolby Vision decoder failed)", HdrFormatPolicy.overlayWord(HdrFormatPolicy.DV_101, false));
        assertEquals("HDR10", HdrFormatPolicy.name(0));
    }
}

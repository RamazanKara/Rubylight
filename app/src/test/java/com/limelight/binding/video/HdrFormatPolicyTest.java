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
    public void anHlgOnlyScreenThatGetsStockHdr10IsNeitherQuietNorReportedAsHdr() {
        // The screen got HDR because the PC offers HLG, then the stream stays PQ: a surface the
        // client draws itself, or a host whose DESCRIBE leaves HLG out (format 0 either way).
        int[] hlgOnly = {HLG};
        assertTrue(HdrPolicy.displayRefusal(33, true, hlgOnly, null, HdrFormatPolicy.ALL) == HdrPolicy.Refusal.NONE);
        for (int format : new int[] {HdrFormatPolicy.pick(HdrFormatPolicy.ALL, hlgOnly, false, true),
                HdrFormatPolicy.pick(HdrFormatPolicy.PQ, hlgOnly, false, false)}) {
            assertEquals(0, format);
            int transfer = HdrFormatPolicy.transfer(format);
            assertEquals(HdrPolicy.Status.PQ_ON_HLG_SCREEN, HdrPolicy.status(true, true, true,
                    HdrPolicy.Metadata.USABLE, null, transfer, HdrPolicy.hlgOnly(hlgOnly)));
            assertFalse(HdrPolicy.reportsHdrOutput(hlgOnly, transfer));
        }
        // The HLG stream it was asked for is quiet and reported as HDR.
        int transfer = HdrFormatPolicy.transfer(HdrFormatPolicy.pick(HdrFormatPolicy.ALL, hlgOnly, false, false));
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
        assertEquals(MediaFormat.COLOR_TRANSFER_ST2084, HdrFormatPolicy.transfer(0));
        assertTrue(HdrFormatPolicy.dolbyVision(HdrFormatPolicy.DV_81));
        assertTrue(HdrFormatPolicy.dolbyVision(HdrFormatPolicy.DV_84));
        assertFalse(HdrFormatPolicy.dolbyVision(HdrFormatPolicy.HLG));
        assertFalse(HdrFormatPolicy.dolbyVision(0));
    }

    @Test
    public void dolbyVisionOnTheAutomaticCodecLeavesAv1OutForHevc() {
        // The TCL C6K's formats: H.264, HEVC Main10 and AV1 Main10. The handshake would take AV1
        // and the ANNOUNCE then drops Dolby Vision for AV1 HDR10.
        int tv = MoonBridge.VIDEO_FORMAT_H264 | MoonBridge.VIDEO_FORMAT_H265 | MoonBridge.VIDEO_FORMAT_H265_MAIN10 |
                MoonBridge.VIDEO_FORMAT_AV1_MAIN8 | MoonBridge.VIDEO_FORMAT_AV1_MAIN10;
        int withoutAv1 = tv & ~MoonBridge.VIDEO_FORMAT_MASK_AV1;
        assertEquals(withoutAv1, HdrFormatPolicy.videoFormats(HdrFormatPolicy.DV_81, tv, true));
        assertEquals(withoutAv1, HdrFormatPolicy.videoFormats(HdrFormatPolicy.DV_84, tv, true));
        // Any other format, a codec the user chose, or no HEVC Main10 to carry it: unchanged.
        for (int format : new int[] {0, HdrFormatPolicy.PQ, HdrFormatPolicy.HLG}) {
            assertEquals(tv, HdrFormatPolicy.videoFormats(format, tv, true));
        }
        assertEquals(tv, HdrFormatPolicy.videoFormats(HdrFormatPolicy.DV_81, tv, false));
        int noHevc10 = tv & ~MoonBridge.VIDEO_FORMAT_H265_MAIN10;
        assertEquals(noHevc10, HdrFormatPolicy.videoFormats(HdrFormatPolicy.DV_81, noHevc10, true));
        assertEquals(withoutAv1, HdrFormatPolicy.videoFormats(HdrFormatPolicy.DV_81, withoutAv1, true));
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

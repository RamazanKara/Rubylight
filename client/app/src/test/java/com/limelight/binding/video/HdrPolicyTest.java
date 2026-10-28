package com.limelight.binding.video;

import static org.junit.Assert.*;

import android.media.MediaFormat;
import android.view.Display;

import com.limelight.binding.video.HdrPolicy.Metadata;
import com.limelight.binding.video.HdrPolicy.Refusal;
import com.limelight.binding.video.HdrPolicy.Status;

import org.junit.Test;

/** Made-up capabilities only: no HDR phone was available, see docs/expected-hdr10.md. */
public class HdrPolicyTest {
    private static final int HDR10 = Display.HdrCapabilities.HDR_TYPE_HDR10;
    private static final int HLG = Display.HdrCapabilities.HDR_TYPE_HLG;
    private static final int DOLBY = Display.HdrCapabilities.HDR_TYPE_DOLBY_VISION;
    private static final int HDR10_PLUS = Display.HdrCapabilities.HDR_TYPE_HDR10_PLUS;
    private static final int[] NO_TYPES = new int[0];

    @Test
    public void hdrIsAskedForOnlyWhenTheSettingIsOnAndTheScreenReportsHdr10() {
        assertEquals(Refusal.NONE, HdrPolicy.displayRefusal(33, true, new int[] {HDR10}, null));
        assertEquals(Refusal.NONE, HdrPolicy.displayRefusal(33, true, new int[] {DOLBY, HDR10, HLG, HDR10_PLUS}, null));
        assertEquals(Refusal.SETTING_OFF, HdrPolicy.displayRefusal(33, false, new int[] {HDR10}, null));
    }

    @Test
    public void aScreenWithoutHdr10IsRefusedWhateverElseItHas() {
        // HLG and Dolby Vision are not what the PC encodes.
        assertEquals(Refusal.DISPLAY_WITHOUT_HDR10, HdrPolicy.displayRefusal(33, true, new int[] {HLG}, null));
        assertEquals(Refusal.DISPLAY_WITHOUT_HDR10, HdrPolicy.displayRefusal(33, true, new int[] {DOLBY, HLG}, null));
        assertEquals(Refusal.DISPLAY_WITHOUT_HDR10, HdrPolicy.displayRefusal(33, true, NO_TYPES, null));
        // Some devices return no capabilities at all (Lenovo Mirage Solo, Android 8.0).
        assertEquals(Refusal.DISPLAY_WITHOUT_HDR10, HdrPolicy.displayRefusal(33, true, null, null));
    }

    @Test
    public void beforeAndroid7NoHdrIsAskedFor() {
        for (int sdk : new int[] {21, 23}) {
            assertEquals(Refusal.ANDROID_TOO_OLD, HdrPolicy.displayRefusal(sdk, true, new int[] {HDR10}, null));
        }
        assertEquals(Refusal.NONE, HdrPolicy.displayRefusal(24, true, new int[] {HDR10}, null));
    }

    @Test
    public void onAndroid14ACapabilityListWithoutHdr10IsAModeProblemWhenAnotherModeHasIt() {
        // From Android 14 the display's capabilities follow the active mode: HDR10 missing there
        // while another mode lists it is a refusal with its own reason, never a request.
        assertEquals(Refusal.MODE_WITHOUT_HDR10,
                HdrPolicy.displayRefusal(34, true, new int[] {HLG}, new int[] {HDR10, HLG}));
        assertEquals(Refusal.MODE_WITHOUT_HDR10, HdrPolicy.displayRefusal(34, true, NO_TYPES, new int[] {HDR10}));
        assertEquals(Refusal.MODE_WITHOUT_HDR10, HdrPolicy.displayRefusal(34, true, null, new int[] {HDR10}));
        // No mode has it: the screen cannot show it at all.
        assertEquals(Refusal.DISPLAY_WITHOUT_HDR10, HdrPolicy.displayRefusal(34, true, new int[] {HLG}, new int[] {HLG}));
        assertEquals(Refusal.DISPLAY_WITHOUT_HDR10, HdrPolicy.displayRefusal(34, true, new int[] {HLG}, NO_TYPES));
        // Older Android has no per-mode list.
        assertEquals(Refusal.DISPLAY_WITHOUT_HDR10, HdrPolicy.displayRefusal(34, true, new int[] {HLG}, null));
        assertEquals(Refusal.DISPLAY_WITHOUT_HDR10,
                HdrPolicy.displayRefusal(33, true, new int[] {HLG}, new int[] {HDR10}));
        // HDR10 in the active mode is the only way to be asked for, whatever the modes say.
        assertEquals(Refusal.NONE, HdrPolicy.displayRefusal(34, true, new int[] {HDR10}, new int[] {HLG}));
        assertEquals(Refusal.NONE, HdrPolicy.displayRefusal(34, true, new int[] {HDR10}, null));
    }

    @Test
    public void aDecoderForHevcAv1OrPyroWaveIsEnough() {
        assertEquals(Refusal.NONE, HdrPolicy.decoderRefusal(true, false, false));
        assertEquals(Refusal.NONE, HdrPolicy.decoderRefusal(false, true, false));
        assertEquals(Refusal.NONE, HdrPolicy.decoderRefusal(false, false, true));
        assertEquals(Refusal.DECODER_WITHOUT_HDR10, HdrPolicy.decoderRefusal(false, false, false));
    }

    @Test
    public void masteringDataIsUsableOnlyWithPrimariesAndAMaximumLuminance() {
        assertEquals(Metadata.ABSENT, HdrPolicy.metadata(null));
        assertEquals(Metadata.ABSENT, HdrPolicy.metadata(new byte[23]));
        assertEquals(Metadata.EMPTY, HdrPolicy.metadata(new byte[24]));
        assertEquals(Metadata.EMPTY, HdrPolicy.metadata(wire(true, 0)));
        assertEquals(Metadata.EMPTY, HdrPolicy.metadata(wire(false, 1000)));
        assertEquals(Metadata.USABLE, HdrPolicy.metadata(wire(true, 1000)));
        // The 26 byte form the host sends carries the full-frame luminance after the 24 bytes.
        byte[] long26 = new byte[26];
        System.arraycopy(wire(true, 4000), 0, long26, 0, 24);
        assertEquals(Metadata.USABLE, HdrPolicy.metadata(long26));
    }

    /** CTA-861.3 without the type byte: 12 bytes of primaries, 4 of white point, then max luminance (LE). */
    private static byte[] wire(boolean primaries, int maxNits) {
        byte[] data = new byte[24];
        if (primaries) {
            data[0] = (byte) 0x34;
            data[1] = (byte) 0x21;
        }
        data[16] = (byte) (maxNits & 0xff);
        data[17] = (byte) (maxNits >> 8);
        return data;
    }

    @Test
    public void aStreamThatDidNotAskForHdrHasNothingToReport() {
        assertEquals(Status.QUIET, HdrPolicy.status(false, false, null, Metadata.ABSENT, null));
        assertEquals(Status.QUIET, HdrPolicy.status(false, true, false, Metadata.ABSENT, 1));
    }

    @Test
    public void hdrAskedForButNotNegotiatedIsNamed() {
        assertEquals(Status.NOT_NEGOTIATED, HdrPolicy.status(true, false, null, Metadata.ABSENT, null));
        // Even when the PC says HDR: with an 8-bit format it would not be HDR10.
        assertEquals(Status.NOT_NEGOTIATED, HdrPolicy.status(true, false, true, Metadata.USABLE,
                MediaFormat.COLOR_TRANSFER_ST2084));
    }

    @Test
    public void thePcsFirstAnswerIsAwaitedAndThenAnSdrAnswerIsNamed() {
        assertEquals(Status.WAITING, HdrPolicy.status(true, true, null, Metadata.ABSENT, null));
        assertEquals(Status.HOST_SDR, HdrPolicy.status(true, true, false, Metadata.ABSENT, null));
    }

    @Test
    public void hdrOnWithGoodMetadataAndAPqOutputIsQuiet() {
        assertEquals(Status.QUIET, HdrPolicy.status(true, true, true, Metadata.USABLE,
                MediaFormat.COLOR_TRANSFER_ST2084));
        // A decoder that leaves the transfer out gives nothing to object to.
        assertEquals(Status.QUIET, HdrPolicy.status(true, true, true, Metadata.USABLE, null));
    }

    @Test
    public void anHlgScreenGetsHdrWhenThePcOffersHlg() {
        assertEquals(Refusal.NONE, HdrPolicy.displayRefusal(33, true, new int[] {HLG}, null, HdrFormatPolicy.HLG));
        assertEquals(Refusal.NONE, HdrPolicy.displayRefusal(33, true, new int[] {HLG}, null, HdrFormatPolicy.ALL));
        // Dolby Vision on the PC does not help a screen without HDR10.
        assertEquals(Refusal.DISPLAY_WITHOUT_HDR10, HdrPolicy.displayRefusal(33, true, new int[] {DOLBY, HLG}, null,
                HdrFormatPolicy.DV_81));
        assertEquals(Refusal.DISPLAY_WITHOUT_HDR10, HdrPolicy.displayRefusal(33, true, NO_TYPES, null,
                HdrFormatPolicy.ALL));
        assertEquals(Refusal.ANDROID_TOO_OLD, HdrPolicy.displayRefusal(23, true, new int[] {HLG}, null,
                HdrFormatPolicy.HLG));
    }

    @Test
    public void hlgDoesNotStandInForHdr10WhenTheVulkanRendererDrawsPqOnly() {
        // Without VK_EXT_swapchain_colorspace the Vulkan renderer's surface is BT.2020/PQ: an
        // HLG-only screen would get HDR10.
        HdrFormatPolicy.DrawnSurface pqOnly = HdrFormatPolicy.DrawnSurface.PQ_ONLY;
        assertEquals(Refusal.HLG_NEEDS_DIRECT_OUTPUT, HdrPolicy.displayRefusal(33, true, new int[] {HLG}, null,
                HdrFormatPolicy.ALL, pqOnly));
        assertEquals(Refusal.NONE, HdrPolicy.displayRefusal(33, true, new int[] {HLG}, null,
                HdrFormatPolicy.ALL, HdrFormatPolicy.DrawnSurface.NONE));
        // A screen with HDR10 does not care what draws the picture.
        assertEquals(Refusal.NONE, HdrPolicy.displayRefusal(33, true, new int[] {HDR10, HLG}, null,
                HdrFormatPolicy.ALL, pqOnly));
        // Without HLG on offer the screen has no HDR at all, whatever the renderer.
        assertEquals(Refusal.DISPLAY_WITHOUT_HDR10, HdrPolicy.displayRefusal(33, true, new int[] {HLG}, null,
                HdrFormatPolicy.PQ, pqOnly));
        // A mode with HDR10 is still the better advice.
        assertEquals(Refusal.MODE_WITHOUT_HDR10, HdrPolicy.displayRefusal(34, true, new int[] {HLG},
                new int[] {HDR10, HLG}, HdrFormatPolicy.ALL, pqOnly));
        assertEquals(Refusal.SETTING_OFF, HdrPolicy.displayRefusal(33, false, new int[] {HLG}, null,
                HdrFormatPolicy.ALL, pqOnly));
    }

    @Test
    public void theVulkanRendererWithHdrColorSpacesTakesAnHlgOnlyScreen() {
        HdrFormatPolicy.DrawnSurface vulkan = HdrFormatPolicy.DrawnSurface.PQ_HLG;
        for (int sdk : new int[] {29, 33, 34, 35}) {
            assertEquals(Refusal.NONE, HdrPolicy.displayRefusal(sdk, true, new int[] {HLG}, null,
                    HdrFormatPolicy.ALL, vulkan));
            assertEquals(Refusal.NONE, HdrPolicy.displayRefusal(sdk, true, new int[] {DOLBY, HLG}, null,
                    HdrFormatPolicy.HLG, vulkan));
        }
        // Nothing else changes: no HLG on offer is still no HDR.
        assertEquals(Refusal.DISPLAY_WITHOUT_HDR10, HdrPolicy.displayRefusal(33, true, new int[] {HLG}, null,
                HdrFormatPolicy.PQ, vulkan));
    }

    @Test
    public void hdrTheVulkanRendererToneMapsIsNamedOverAQuietStatus() {
        assertEquals(Status.SHOWN_AS_SDR, HdrPolicy.drawnStatus(Status.QUIET, true));
        assertEquals(Status.SHOWN_AS_SDR, HdrPolicy.drawnStatus(Status.NO_METADATA, true));
        // What the PC or the negotiation says comes first, and nothing changes without tone mapping.
        for (Status status : new Status[] {Status.WAITING, Status.HOST_SDR, Status.NOT_NEGOTIATED,
                Status.PQ_ON_HLG_SCREEN, Status.OUTPUT_NOT_PQ}) {
            assertEquals(status, HdrPolicy.drawnStatus(status, true));
        }
        for (Status status : Status.values()) {
            assertEquals(status, HdrPolicy.drawnStatus(status, false));
        }
    }

    @Test
    public void theHlgOnlyDebugHookCountsOnlyInADebugBuild() {
        int[] tv = {DOLBY, HDR10, HLG, HDR10_PLUS};
        try {
            HdrPolicy.setDebugHlgOnly(false, true);
            assertSame(tv, HdrPolicy.effectiveHdrTypes(tv));
            HdrPolicy.setDebugHlgOnly(true, false);
            assertSame(tv, HdrPolicy.effectiveHdrTypes(tv));

            HdrPolicy.setDebugHlgOnly(true, true);
            int[] hlgOnly = HdrPolicy.effectiveHdrTypes(tv);
            assertArrayEquals(new int[] {HLG}, hlgOnly);
            assertTrue(HdrPolicy.hlgOnly(hlgOnly));
            assertArrayEquals(new int[] {HLG}, HdrPolicy.effectiveHdrTypes(new int[] {HDR10}));
            // A screen without HDR stays without it.
            assertSame(NO_TYPES, HdrPolicy.effectiveHdrTypes(NO_TYPES));
            assertNull(HdrPolicy.effectiveHdrTypes(null));
            assertArrayEquals(new int[] {DOLBY}, HdrPolicy.effectiveHdrTypes(new int[] {DOLBY}));
        } finally {
            HdrPolicy.setDebugHlgOnly(false, false);
        }
    }

    @Test
    public void aPqStreamOnAnHlgOnlyScreenIsNamed() {
        int hlg = MediaFormat.COLOR_TRANSFER_HLG;
        int pq = MediaFormat.COLOR_TRANSFER_ST2084;
        assertTrue(HdrPolicy.hlgOnly(new int[] {HLG}));
        assertTrue(HdrPolicy.hlgOnly(new int[] {DOLBY, HLG}));
        assertFalse(HdrPolicy.hlgOnly(new int[] {HDR10, HLG}));
        assertFalse(HdrPolicy.hlgOnly(NO_TYPES));
        assertFalse(HdrPolicy.hlgOnly(null));
        // HLG was the reason HDR was asked for, but the ANNOUNCE carried none: named before
        // anything the PC reports, and with no mastering data asked of it.
        assertEquals(Status.PQ_ON_HLG_SCREEN, HdrPolicy.status(true, true, null, Metadata.ABSENT, null, pq, true));
        assertEquals(Status.PQ_ON_HLG_SCREEN, HdrPolicy.status(true, true, true, Metadata.USABLE, pq, pq, true));
        assertEquals(Status.QUIET, HdrPolicy.status(true, true, true, Metadata.ABSENT, hlg, hlg, true));
        // A stream that did not negotiate 10-bit says that instead.
        assertEquals(Status.NOT_NEGOTIATED, HdrPolicy.status(true, false, null, Metadata.ABSENT, null, pq, true));
        assertEquals(Status.QUIET, HdrPolicy.status(false, true, true, Metadata.USABLE, pq, pq, true));
    }

    @Test
    public void anHlgOnlyScreenIsReportedAsHdrOnlyForAnHlgStream() {
        int hlg = MediaFormat.COLOR_TRANSFER_HLG;
        int pq = MediaFormat.COLOR_TRANSFER_ST2084;
        assertTrue(HdrPolicy.reportsHdrOutput(new int[] {HLG}, hlg));
        assertFalse(HdrPolicy.reportsHdrOutput(new int[] {HLG}, pq));
        assertTrue(HdrPolicy.reportsHdrOutput(new int[] {HDR10}, pq));
        assertTrue(HdrPolicy.reportsHdrOutput(new int[] {HDR10, HLG}, hlg));
        assertFalse(HdrPolicy.reportsHdrOutput(new int[] {DOLBY}, pq));
        assertFalse(HdrPolicy.reportsHdrOutput(NO_TYPES, hlg));
        assertFalse(HdrPolicy.reportsHdrOutput(null, hlg));
    }

    @Test
    public void anHlgStreamExpectsAnHlgOutputAndNoMasteringData() {
        int hlg = MediaFormat.COLOR_TRANSFER_HLG;
        assertEquals(Status.QUIET, HdrPolicy.status(true, true, true, Metadata.ABSENT, hlg, hlg));
        assertEquals(Status.QUIET, HdrPolicy.status(true, true, true, Metadata.ABSENT, null, hlg));
        assertEquals(Status.OUTPUT_NOT_PQ, HdrPolicy.status(true, true, true, Metadata.USABLE,
                MediaFormat.COLOR_TRANSFER_ST2084, hlg));
        // And a PQ stream still objects to an HLG output.
        assertEquals(Status.OUTPUT_NOT_PQ, HdrPolicy.status(true, true, true, Metadata.USABLE, hlg));
    }

    @Test
    public void hdrOnWithoutUsableMasteringDataIsNamed() {
        assertEquals(Status.NO_METADATA, HdrPolicy.status(true, true, true, Metadata.ABSENT, null));
        assertEquals(Status.NO_METADATA, HdrPolicy.status(true, true, true, Metadata.EMPTY,
                MediaFormat.COLOR_TRANSFER_ST2084));
    }

    @Test
    public void aDecoderOutputThatIsNotPqIsNamedBeforeTheMetadata() {
        assertEquals(Status.OUTPUT_NOT_PQ, HdrPolicy.status(true, true, true, Metadata.USABLE,
                MediaFormat.COLOR_TRANSFER_SDR_VIDEO));
        assertEquals(Status.OUTPUT_NOT_PQ, HdrPolicy.status(true, true, true, Metadata.ABSENT,
                MediaFormat.COLOR_TRANSFER_HLG));
    }
}

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

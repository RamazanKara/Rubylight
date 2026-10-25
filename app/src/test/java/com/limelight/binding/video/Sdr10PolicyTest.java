package com.limelight.binding.video;

import org.junit.Test;

import static com.limelight.binding.video.Sdr10Policy.Refusal;
import static com.limelight.nvstream.jni.MoonBridge.*;
import static org.junit.Assert.*;

public class Sdr10PolicyTest {
    private static Refusal refusal(boolean media, boolean enabled, boolean hdr, boolean hevc, boolean av1,
                                   boolean pyroWave, boolean packed) {
        return Sdr10Policy.refusal(media, enabled, hdr, hevc, av1, pyroWave, packed);
    }

    @Test
    public void nativeFullChromaOnlyWinsWithAFormatThePcAlsoSupports() {
        assertTrue(Sdr10Policy.yuv444Wins(VIDEO_FORMAT_H264 | VIDEO_FORMAT_H264_HIGH8_444));
        assertTrue(Sdr10Policy.yuv444Wins(VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_REXT8_444 | VIDEO_FORMAT_H265_MAIN10));
        // The PC has no 4:4:4 in common: 10-bit SDR stays.
        assertFalse(Sdr10Policy.yuv444Wins(VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10));
        assertFalse(Sdr10Policy.yuv444Wins(0));
        // The 10-bit formats go and the 8-bit ones stay.
        assertEquals(VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_REXT8_444 | VIDEO_FORMAT_AV1_MAIN8,
                Sdr10Policy.withoutTenBit(VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10 |
                        VIDEO_FORMAT_H265_REXT8_444 | VIDEO_FORMAT_AV1_MAIN8 | VIDEO_FORMAT_AV1_MAIN10));
    }

    @Test
    public void theHostHdrNoticeSaysOnceAndRearmsWhenThePcReturnsToSdr() {
        // Only a 10-bit stream, and only a changed report, does anything.
        assertEquals(Sdr10Policy.HostHdrStep.NONE, Sdr10Policy.hostHdrStep(false, true, null));
        assertEquals(Sdr10Policy.HostHdrStep.NONE, Sdr10Policy.hostHdrStep(true, null, null));
        assertEquals(Sdr10Policy.HostHdrStep.SAY, Sdr10Policy.hostHdrStep(true, true, null));
        assertEquals(Sdr10Policy.HostHdrStep.NONE, Sdr10Policy.hostHdrStep(true, true, true));
        assertEquals(Sdr10Policy.HostHdrStep.REARM, Sdr10Policy.hostHdrStep(true, false, true));
        assertEquals(Sdr10Policy.HostHdrStep.NONE, Sdr10Policy.hostHdrStep(true, false, false));
        assertEquals(Sdr10Policy.HostHdrStep.SAY, Sdr10Policy.hostHdrStep(true, true, false));
        // The first report being SDR also clears a claim left by an earlier stream.
        assertEquals(Sdr10Policy.HostHdrStep.REARM, Sdr10Policy.hostHdrStep(true, false, null));
    }

    @Test
    public void theDecoderChoiceChangesOnlyWhenTenBitSdrCanBeOffered() {
        assertTrue(Sdr10Policy.decoderChoiceMayChange(true, false, false, false));
        assertFalse(Sdr10Policy.decoderChoiceMayChange(false, false, false, false));
        assertFalse(Sdr10Policy.decoderChoiceMayChange(true, true, false, false));
        assertFalse(Sdr10Policy.decoderChoiceMayChange(true, false, true, false));
        assertFalse(Sdr10Policy.decoderChoiceMayChange(true, false, false, true));
    }

    @Test
    public void settingOffOffersNothing() {
        Refusal r = refusal(true, false, false, true, true, false, false);
        assertEquals(Refusal.SETTING_OFF, r);
        assertEquals(0, Sdr10Policy.formats(r, true, true));
        assertFalse(Sdr10Policy.worthSaying(r));
    }

    @Test
    public void main10DecodersOfferHevcAndAv1TenBitWithoutPyroWave() {
        Refusal r = refusal(true, true, false, true, true, false, false);
        assertEquals(Refusal.NONE, r);
        int formats = Sdr10Policy.formats(r, true, true);
        assertEquals(VIDEO_FORMAT_H265_MAIN10 | VIDEO_FORMAT_AV1_MAIN10, formats);
        assertEquals(0, formats & VIDEO_FORMAT_MASK_PYROWAVE);
        assertEquals(VIDEO_FORMAT_H265_MAIN10, Sdr10Policy.formats(Refusal.NONE, true, false));
        assertEquals(VIDEO_FORMAT_AV1_MAIN10, Sdr10Policy.formats(Refusal.NONE, false, true));
    }

    @Test
    public void noMain10DecoderRefusesAndSaysSo() {
        Refusal r = refusal(true, true, false, false, false, false, false);
        assertEquals(Refusal.NO_MAIN10, r);
        assertEquals(0, Sdr10Policy.formats(r, false, false));
        assertTrue(Sdr10Policy.worthSaying(r));
    }

    @Test
    public void grantedHdrWinsAndChangesNothing() {
        Refusal r = refusal(true, true, true, true, true, false, false);
        assertEquals(Refusal.HDR_STREAM, r);
        assertEquals(0, Sdr10Policy.formats(r, true, true));
        assertFalse(Sdr10Policy.worthSaying(r));
    }

    @Test
    public void pyroWaveAndPackedAndInputOnlyStreamsAreNotOffered() {
        assertEquals(Refusal.PYROWAVE, refusal(true, true, false, true, true, true, false));
        assertEquals(Refusal.PACKED_444, refusal(true, true, false, true, true, false, true));
        assertEquals(Refusal.NO_VIDEO, refusal(false, true, false, true, true, false, false));
        assertTrue(Sdr10Policy.worthSaying(Refusal.PYROWAVE));
        assertTrue(Sdr10Policy.worthSaying(Refusal.PACKED_444));
        assertFalse(Sdr10Policy.worthSaying(Refusal.NO_VIDEO));
    }

    @Test
    public void aHostReportingHdrIsNamedNotHidden() {
        assertEquals(Sdr10Policy.Status.HOST_HDR, Sdr10Policy.status(true, true));
        assertEquals(Sdr10Policy.Status.QUIET, Sdr10Policy.status(true, false));
        assertEquals(Sdr10Policy.Status.QUIET, Sdr10Policy.status(true, null));
        assertEquals(Sdr10Policy.Status.QUIET, Sdr10Policy.status(false, true));
    }

    @Test
    public void overlayLabelsTenBitSdrAndStillSaysHdr() {
        assertEquals("10-bit SDR", Sdr10Policy.overlayColor(true, true, null));
        assertEquals("10-bit SDR", Sdr10Policy.overlayColor(true, true, false));
        assertEquals("HDR", Sdr10Policy.overlayColor(true, true, true));
        assertEquals("SDR", Sdr10Policy.overlayColor(false, true, false));
        assertEquals("10-bit", Sdr10Policy.overlayColor(true, false, null));
        assertEquals("SDR", Sdr10Policy.overlayColor(true, false, false));
        assertEquals("HDR", Sdr10Policy.overlayColor(true, false, true));
    }

    @Test
    public void tenBitSdrStartsAsSdrSoTheFirstReportDoesNotRestartTheDecoder() {
        assertFalse(Sdr10Policy.assumeHdr(true, true));
        assertTrue(Sdr10Policy.assumeHdr(true, false));
        assertFalse(Sdr10Policy.assumeHdr(false, false));
    }
}

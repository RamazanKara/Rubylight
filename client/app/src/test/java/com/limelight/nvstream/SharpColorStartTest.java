package com.limelight.nvstream;

import android.app.Application;

import com.limelight.binding.video.SharpColor;
import com.limelight.nvstream.jni.MoonBridge;

import org.json.JSONObject;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import static org.junit.Assert.*;

/** What a start asks Sharper colored text, and the formats each pick leaves it to offer. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class SharpColorStartTest {
    private static final int HEVC_444 = MoonBridge.VIDEO_FORMAT_H264 | MoonBridge.VIDEO_FORMAT_H264_HIGH8_444 |
            MoonBridge.VIDEO_FORMAT_H265 | MoonBridge.VIDEO_FORMAT_H265_REXT8_444 | MoonBridge.VIDEO_FORMAT_PYROWAVE |
            MoonBridge.VIDEO_FORMAT_PYROWAVE_444;

    @Test
    public void theRequestSplitsTheOfferIntoBaseAndWhatThisDeviceDecodes() {
        SharpColor.Request request = NvConnection.sharpColorRequest(HEVC_444, false, false, true, 0x80101, 3, false);
        assertTrue(request.auto);
        assertFalse(request.hdr);
        // PyroWave's 4:4:4 stays the codec's own; only the native 4:4:4 formats are decodable.
        assertEquals(MoonBridge.VIDEO_FORMAT_H264 | MoonBridge.VIDEO_FORMAT_H265 | MoonBridge.VIDEO_FORMAT_PYROWAVE |
                MoonBridge.VIDEO_FORMAT_PYROWAVE_444, request.base);
        assertEquals(MoonBridge.VIDEO_FORMAT_H264_HIGH8_444 | MoonBridge.VIDEO_FORMAT_H265_REXT8_444, request.decodable);
        assertEquals("", request.decoderReason);
        assertEquals(3, request.hostPacked);
    }

    @Test
    public void tenBitFormatsAreHdrUnless10BitSdrAddedThem() {
        int offered = MoonBridge.VIDEO_FORMAT_H264 | MoonBridge.VIDEO_FORMAT_H265 | MoonBridge.VIDEO_FORMAT_H265_MAIN10;
        SharpColor.Request hdr = NvConnection.sharpColorRequest(offered, false, false, true, 0x301, -1, false);
        assertTrue(hdr.hdr);
        assertEquals(offered, hdr.base);
        assertNotEquals("", hdr.decoderReason);
        SharpColor.Request sdr10 = NvConnection.sharpColorRequest(offered, true, false, true, 0x301, -1, false);
        assertFalse(sdr10.hdr);
        // 10-bit SDR's formats are the decision's to give, so they leave the base.
        assertEquals(MoonBridge.VIDEO_FORMAT_H264 | MoonBridge.VIDEO_FORMAT_H265, sdr10.base);
        assertEquals(MoonBridge.VIDEO_FORMAT_H265_MAIN10, sdr10.decodable);
    }

    @Test
    public void theClassicEngineNeverReportsPackedStateItCannotUse() {
        SharpColor.Request request = NvConnection.sharpColorRequest(HEVC_444, false, false, false, 0x80101, 3, true);
        assertFalse(request.packedEngine);
        assertEquals(-1, request.hostPacked);
        assertFalse(request.packedRefusedBefore);
    }

    @Test
    public void eachPickLeavesItsFormats() throws Exception {
        int offered = HEVC_444 | MoonBridge.VIDEO_FORMAT_H265_MAIN10;
        SharpColor.Pick nativePick = SharpColor.read(new JSONObject().put("pick", "native").put("formats", 0x501));
        assertEquals(0x501, NvConnection.formatsAfter(nativePick, offered, false));
        // Packed streams HEVC or AV1 4:2:0; 10-bit only for HDR.
        SharpColor.Pick packed = SharpColor.read(new JSONObject().put("pick", "packed").put("modes", 1));
        assertEquals(MoonBridge.VIDEO_FORMAT_H265, NvConnection.formatsAfter(packed, offered, false));
        assertEquals(MoonBridge.VIDEO_FORMAT_H265 | MoonBridge.VIDEO_FORMAT_H265_MAIN10,
                NvConnection.formatsAfter(packed, offered, true));
        // Neither: the native 4:4:4 formats go, everything else (HDR, PyroWave 4:4:4) stays.
        SharpColor.Pick without = SharpColor.read(new JSONObject().put("pick", "without").put("why", "hdr"));
        assertEquals(offered & ~(MoonBridge.VIDEO_FORMAT_H264_HIGH8_444 | MoonBridge.VIDEO_FORMAT_H265_REXT8_444),
                NvConnection.formatsAfter(without, offered, true));
    }
}

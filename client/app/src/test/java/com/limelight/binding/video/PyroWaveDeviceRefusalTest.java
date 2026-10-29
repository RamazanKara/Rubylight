package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;

import com.limelight.R;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

/** PyroWave chosen on a device that cannot run it: the renderer keeps the reason for the stream's notice. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class PyroWaveDeviceRefusalTest {
    @Test
    public void aDeviceWithoutThePyroWaveRendererSaysWhy() {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.videoFormat = PreferenceConfiguration.FormatOption.FORCE_PYROWAVE;
            MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity.get(), prefs, null, 0,
                    false, false, "", null);
            // The JVM has no libpyrowave-renderer.so, as a device that can never run it (32-bit, or
            // an app without the library): the reason is the one the settings list shows.
            assertFalse(renderer.isPyroWaveSupported());
            int reason = PyroWaveDecoderRenderer.neverAvailableReason();
            assertTrue(reason == R.string.pyrowave_decoder_unavailable || reason == R.string.pyrowave_needs_64_bit);
            assertEquals(reason, (int) ReflectionHelpers.getField(renderer, "pyroWaveUnavailableReason"));
        }
    }

    @Test
    public void theDropReasonBlamesThePcOnlyWhenItsCodecsLackPyroWave() {
        int hdr = MoonBridge.VIDEO_FORMAT_PYROWAVE_MAIN10;
        int sdr = MoonBridge.VIDEO_FORMAT_PYROWAVE;
        int both = sdr | hdr | MoonBridge.VIDEO_FORMAT_H265 | MoonBridge.VIDEO_FORMAT_H265_MAIN10;
        int hevcHdr = MoonBridge.VIDEO_FORMAT_H265 | MoonBridge.VIDEO_FORMAT_H265_MAIN10;
        // This device cannot run it: its reason, whatever the PC offers.
        assertEquals(R.string.pyrowave_needs_64_bit, drop(R.string.pyrowave_needs_64_bit, PC_ALL, both, 0, 0));
        // The device can and the PC's codecs have none.
        assertEquals(MediaCodecDecoderRenderer.PYROWAVE_NOT_OFFERED, drop(0, PC_NONE, hevcHdr, 0, 0));
        // Both offer it and no surface could be set up: the device's display, not the PC.
        assertEquals(R.string.pyrowave_missing_swapchain, drop(0, PC_ALL, both, 0, R.string.pyrowave_hdr_display_refused));
        // The HDR surface failed and SDR PyroWave is ready: the HDR failure, if HDR takes another codec.
        assertEquals(R.string.pyrowave_hdr_display_refused, drop(0, PC_ALL, both, sdr, R.string.pyrowave_hdr_display_refused));
        assertEquals(0, drop(0, PC_ALL, both, hdr, 0));
        assertEquals(0, MediaCodecDecoderRenderer.pyroWaveDropReason(false, false, 0, PC_NONE, hevcHdr, 0, 0));
    }

    @Test
    public void anHdrStreamOnAPcWithSdrPyroWaveOnlySaysThePcHasNoPyroWaveHdr() {
        // Negotiation lets HDR in HEVC win over SDR-only PyroWave: the PC has PyroWave, not in HDR.
        int hevcHdr = MoonBridge.VIDEO_FORMAT_H265 | MoonBridge.VIDEO_FORMAT_H265_MAIN10;
        assertEquals(MediaCodecDecoderRenderer.PYROWAVE_NO_HDR_ON_PC, drop(0, PC_SDR, hevcHdr, 0, 0));
        // An SDR stream on the same PC had PyroWave; left out anyway, it is no one's known fault.
        assertEquals(0, drop(0, PC_SDR, MoonBridge.VIDEO_FORMAT_H265, 0, 0));
    }

    @Test
    public void fullChromaOnSaysSoInsteadOfBlamingThePc() {
        // Packed 4:4:4 never offers PyroWave, so the common formats have none on any PC.
        assertEquals(MediaCodecDecoderRenderer.PYROWAVE_FULL_CHROMA, MediaCodecDecoderRenderer.pyroWaveDropReason(
                true, true, 0, PC_ALL, MoonBridge.VIDEO_FORMAT_H265, 0, 0));
    }

    // ServerCodecModeSupport: other codecs (0x300) and the PyroWave bits.
    private static final int PC_NONE = 0x300;
    private static final int PC_SDR = PC_NONE | 0x800000 | 0x1000000;
    private static final int PC_ALL = PC_SDR | 0x2000000 | 0x4000000;

    private static int drop(int deviceReason, int pcCodecs, int formats, int choice, int hdrFailure) {
        return MediaCodecDecoderRenderer.pyroWaveDropReason(true, false, deviceReason, pcCodecs, formats, choice,
                hdrFailure);
    }

    @Test
    public void anotherCodecChosenHasNoReason() {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(activity.get(),
                    new PreferenceConfiguration(), null, 0, false, false, "", null);
            assertFalse(renderer.isPyroWaveSupported());
            assertEquals(0, (int) ReflectionHelpers.getField(renderer, "pyroWaveUnavailableReason"));
        }
    }
}

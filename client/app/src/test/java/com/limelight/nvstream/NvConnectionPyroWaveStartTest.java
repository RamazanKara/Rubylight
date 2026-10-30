package com.limelight.nvstream;

import android.app.Application;

import com.limelight.nvstream.jni.MoonBridge;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.annotation.Config;

import static org.junit.Assert.assertEquals;

/**
 * The bitrate a stream starts at once the host chose its format: PyroWave's own when it streams
 * PyroWave, whether picked or reached from Auto (the decision keys on the negotiated format).
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class NvConnectionPyroWaveStartTest {
    private static final int HEVC = MoonBridge.VIDEO_FORMAT_H265;
    private static final int PYROWAVE = MoonBridge.VIDEO_FORMAT_PYROWAVE;

    private static int start(int negotiated, int width, int height, int fps, int bitrate, boolean automatic, int limit) {
        ConnectionContext context = new ConnectionContext();
        context.streamConfig = new StreamConfiguration.Builder()
                .setResolution(width, height).setRefreshRate(fps).setBitrate(bitrate)
                .setAutomaticBitrate(automatic).setBitrateLimit(limit).build();
        context.negotiatedVideoFormats = negotiated;
        context.negotiatedWidth = width;
        context.negotiatedHeight = height;
        return NvConnection.pyroWaveStartKbps(context);
    }

    @Test
    public void anAutomaticPyroWaveStreamStartsAtPyroWavesDefault() {
        // The automatic bitrate arrives as the H.264/HEVC default for the mode.
        assertEquals(398132, start(PYROWAVE, 1920, 1080, 60, 20000, true, 0));
        assertEquals(1415578, start(PYROWAVE, 2560, 1440, 120, 70000, true, 0));
        assertEquals(1592525, start(PYROWAVE, 3840, 2160, 60, 80000, true, 0));
        assertEquals(1592525, start(MoonBridge.VIDEO_FORMAT_PYROWAVE_MAIN10, 3840, 2160, 60, 80000, true, 0));
    }

    @Test
    public void aStreamThatFellBackToHevcKeepsItsBitrate() {
        assertEquals(20000, start(HEVC, 1920, 1080, 60, 20000, true, 0));
        assertEquals(20000, start(HEVC, 1920, 1080, 60, 20000, false, 0));
    }

    @Test
    public void aChosenBitrateIsRaisedOnlyBelowTheFloor() {
        assertEquals(186624, start(PYROWAVE, 1920, 1080, 60, 20000, false, 0));
        assertEquals(500000, start(PYROWAVE, 1920, 1080, 60, 500000, false, 0));
        assertEquals(663552, start(PYROWAVE, 2560, 1440, 120, 100000, false, 0));
        assertEquals(900000, start(PYROWAVE, 2560, 1440, 120, 900000, false, 0));
        assertEquals(1000000, start(PYROWAVE, 3840, 2160, 60, 1000000, false, 0));
    }

    @Test
    public void theMobileDataLimitStaysACeiling() {
        assertEquals(50000, start(PYROWAVE, 1920, 1080, 60, 20000, true, 50000));
        assertEquals(100000, start(PYROWAVE, 1920, 1080, 60, 20000, false, 100000));
        // A chosen rate above the limit was the user's own call before; it stays.
        assertEquals(300000, start(PYROWAVE, 1920, 1080, 60, 300000, false, 100000));
    }
}

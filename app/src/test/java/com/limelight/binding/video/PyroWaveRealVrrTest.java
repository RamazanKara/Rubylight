package com.limelight.binding.video;

import org.junit.Test;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

/**
 * Real VRR for PyroWave needs a screen with adaptive refresh and a stream, so this pins what joins
 * the setting to the presenter. Which settings choose the mode is FramePacingPolicyTest's, and the
 * presenter's plan in that mode is pyrowave_present_timing_test.cpp's (scripts/test-native.sh).
 */
public class PyroWaveRealVrrTest {
    private static final String DECODER = "src/main/java/com/limelight/binding/video/MediaCodecDecoderRenderer.java";
    private static final String JAVA = "src/main/java/com/limelight/binding/video/PyroWaveDecoderRenderer.java";
    private static final String NATIVE = "src/main/jni/pyrowave-renderer/pyrowave_renderer.cpp";
    private static final String TIMING = "src/main/jni/pyrowave-renderer/present_timing.h";

    private static String read(String path) throws Exception {
        return new String(Files.readAllBytes(Paths.get(path)), StandardCharsets.UTF_8).replace("\r\n", "\n");
    }

    // Java's mode number is the presenter's: a different one would be another mode, or none
    // (nativeCreate turns a number it does not know into frames as they arrive).
    @Test
    public void javasModeIsThePresentersHostTimesMode() throws Exception {
        Matcher mode = Pattern.compile("enum class Mode \\{[^}]*\\bHostTimes = (\\d+)").matcher(read(TIMING));
        assertTrue(mode.find());
        assertEquals(FramePacingPolicy.PYROWAVE_HOST_TIMES, Integer.parseInt(mode.group(1)));
        String cpp = read(NATIVE);
        assertTrue(cpp.contains("static constexpr int HOST_TIMES_MODE = int(PyroWavePresentTiming::Mode::HostTimes);"));
        assertTrue(cpp.contains("framePacingMode = pacingMode >= 0 && pacingMode <= HOST_TIMES_MODE ? pacingMode : 0;"));
    }

    // The presenter is made before the stream starts: its mode comes from the stream's switch and
    // its display's adaptive refresh, what MediaCodec's clock is started with, and the display's
    // top rate goes along for the shortest time between two frames.
    @Test
    public void thePresenterIsMadeWithTheStreamsSwitchAndItsDisplaysAdaptiveRefresh() throws Exception {
        String decoder = read(DECODER);
        int adaptive = decoder.indexOf("adaptive = outputHasArrSupport;");
        int mode = decoder.indexOf("int pyroWavePacing = FramePacingPolicy.pyroWaveMode(prefs.framePacing, prefs.vrr, "
                + "prefs.hostFrameTimes, adaptive);");
        int top = decoder.indexOf("float topRefreshRate = pyroWavePacing == FramePacingPolicy.PYROWAVE_HOST_TIMES ? "
                + "maxDisplayRefreshRate() : 0;");
        int setup = decoder.indexOf("pyroWaveRenderer.setup(renderTarget, candidate, width, height, fps, displayRefreshRate,\n"
                + "                    topRefreshRate, pyroWavePacing, prefs.fullRange, prefs.stretchVideo,");
        assertTrue(adaptive > 0 && mode > adaptive && top > mode && setup > top);
        // MediaCodec's clock still reads the same two where it did.
        assertTrue(decoder.contains("hostFrameTimes = prefs.hostFrameTimes;"));
        assertTrue(decoder.contains("hostAnnouncesClockSync(), trackContentRate || hostFrameTimes, hostFrameTimes && adaptive);"));

        String java = read(JAVA);
        assertTrue(java.contains("nativeCreate(surface, width, height, fps, displayRefreshRate, topRefreshRate, pacingMode,"));
        assertTrue(java.contains("float displayRefreshRate,\n"
                + "                                           float topRefreshRate, int pacingMode,"));
        assertTrue(read(NATIVE).contains("jfloat displayRefreshRate,\n"
                + "                                                                      jfloat topRefreshRate, jint pacingMode,"));
    }

    // The schedule is the shared clock's, set up as MediaCodec's is for adaptive refresh
    // (android_present_clock.c), and the clock lives as long as the presenter.
    @Test
    public void thePresenterAsksItsOwnSharedClock() throws Exception {
        String cpp = read(NATIVE);
        int settings = cpp.indexOf("rp_present_settings_adaptive(&settings);");
        int interval = cpp.indexOf("settings.min_interval_us = topRefreshRate > 1.0f && topRefreshRate <= 1000.0f ?");
        int made = cpp.indexOf("presentClock = rp_present_clock_new(&settings);");
        int schedule = cpp.indexOf("rp_present_clock_schedule(static_cast<RpPresentClock *>(clock), rtp, readyUs).present_us;");
        assertTrue(settings > 0 && interval > settings && made > interval && schedule > made);
        // A new swapchain starts the clock over with the presenter's own timing.
        assertTrue(cpp.contains("presentTiming.reset();\n"));
        int reset = cpp.indexOf("if (presentClock != nullptr) rp_present_clock_reset(presentClock);");
        assertTrue(reset > cpp.indexOf("presentTiming.reset();\n"));
        assertTrue(cpp.contains("rp_present_clock_free(presentClock);"));
        // The frame's ready time is its decode completion, on the clock the presenter waits on.
        assertTrue(cpp.contains("presentTiming.plan(slot.hostPresentationTimeUs, slot.completedNs);"));
        // Frames stay in order: only frames shown as they arrive give way to a newer one.
        assertTrue(cpp.contains("if (framePacingMode == 0 || slots[index].hostPresentationTimeUs <= 0) {"));
    }

    // The overlay names the mode in the decoder line and says how long frames were held.
    @Test
    public void theOverlayNamesTheModeAndTheHold() throws Exception {
        assertTrue(read(NATIVE).contains("if (renderer->showsHostTimes()) mode += \", PC frame times\";"));
        assertEquals("PC frame times: held 2.3 ms", PyroWaveDecoderRenderer.hostTimesLine(2.34f));
        assertEquals("PC frame times: held 0.0 ms", PyroWaveDecoderRenderer.hostTimesLine(0));
        // Every other mode, and before the first interval: no line.
        assertNull(PyroWaveDecoderRenderer.hostTimesLine(-1));
        assertNull(PyroWaveDecoderRenderer.hostTimesLine(Float.NaN));
    }
}

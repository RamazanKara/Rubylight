package com.limelight.binding.video;

import org.junit.Test;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.List;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

/** PyroWave's renderer needs a GPU and a stream, so this pins its JNI exports and how Frame Warp reaches it. */
public class PyroWaveFrameWarpWiringTest {
    private static final String JAVA = "src/main/java/com/limelight/binding/video/PyroWaveDecoderRenderer.java";
    private static final String NATIVE = "src/main/jni/pyrowave-renderer/pyrowave_renderer.cpp";

    private static String read(String path) throws Exception {
        return new String(Files.readAllBytes(Paths.get(path)), StandardCharsets.UTF_8);
    }

    // A native without its export fails only when a stream first calls it
    @Test
    public void everyPyroWaveNativeHasItsExport() throws Exception {
        String java = read(JAVA);
        String cpp = read(NATIVE);
        List<String> natives = new ArrayList<>();
        Matcher declared = Pattern.compile("private static native \\w+(?:\\[\\])? (\\w+)\\(").matcher(java);
        while (declared.find()) natives.add(declared.group(1));
        assertTrue(natives.contains("nativeSetFrameWarp"));
        assertTrue(natives.contains("nativeNoteFrameNumber"));
        for (String name : natives) {
            assertTrue(name, cpp.contains("Java_com_limelight_binding_video_PyroWaveDecoderRenderer_" + name + "("));
        }
        assertTrue(java.contains("private static native boolean nativeSetFrameWarp(long handle, long api);"));
        assertTrue(cpp.contains("PyroWaveDecoderRenderer_nativeSetFrameWarp(JNIEnv *, jclass, jlong handle,\n" +
                "                                                                          jlong api) {"));
        assertTrue(java.contains("private static native void nativeNoteFrameNumber(long handle, int frame);"));
        assertTrue(cpp.contains("PyroWaveDecoderRenderer_nativeNoteFrameNumber(JNIEnv *, jclass, jlong handle,\n" +
                "                                                                             jint frame) {"));
    }

    // The renderer is made before the stream says it offers Frame Warp: it takes it once the decoder is set
    // up, and each picture's number reaches it with the picture
    @Test
    public void frameWarpReachesPyroWaveAfterSetupWithEachFramesNumber() throws Exception {
        String decoder = read("src/main/java/com/limelight/binding/video/MediaCodecDecoderRenderer.java");
        int check = decoder.indexOf("result = pyroWaveRenderer.getFormat() == format ? 0 : -1;");
        int enable = decoder.indexOf("pyroWaveRenderer.setFrameWarp(MoonBridge.frameWarpApi())");
        assertTrue(check > 0 && enable > check);
        assertTrue(decoder.contains("pyroWaveRenderer.submitFrame(decodeUnitData, directInput, decodeUnitLength, ptsUs,\n" +
                "                frameNumber, presentationTimeUs,"));
        assertFalse(decoder.contains("frame_warp_reason_pyrowave"));
        String java = read(JAVA);
        assertTrue(java.contains("if (frameWarp) nativeNoteFrameNumber(handle, frameNumber);"));
    }

    // Without Frame Warp nothing new runs: the plain pipelines, no set 1, no redraws
    @Test
    public void withoutFrameWarpThePlainPassesDraw() throws Exception {
        String cpp = read(NATIVE);
        assertTrue(cpp.contains("warp == nullptr ? upscalePipeline : warpUpscalePipeline"));
        assertTrue(cpp.contains("warp == nullptr ? pipeline : warpPipeline"));
        assertTrue(cpp.contains("if (schedule.held() >= 0 && warpRedraws()) {"));
        // Only Frame Warp holds a picture for redraws
        assertTrue(cpp.contains("if (!warpRedraws()) return;\n            std::lock_guard<std::mutex> lock(slotMutex);\n" +
                "            schedule.hold(index);"));
    }
}

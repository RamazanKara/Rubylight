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

    // Both passes take the shift the same way: no mask read without a shift or where the mask does not apply, the
    // picture mirrored and faded past its edges, and a pixel whose shifted position is HUD left in place (as the
    // Vulkan, GLES and desktop presenters)
    @Test
    public void bothPassesShiftMirrorAndFadeAlike() throws Exception {
        String warp = read("src/main/jni/pyrowave-renderer/shaders/frame_warp.glsl");
        int zero = warp.indexOf("if (frameWarp.shift.x == 0.0 && frameWarp.shift.y == 0.0) return uv;");
        int masked = warp.indexOf("if (frameWarp.shift.z > 0.5) {\n        if (texture(u_hud, uv).r > 0.5) return uv;");
        assertTrue(zero > 0 && masked > zero);
        assertTrue(warp.indexOf("texture(u_hud") > masked);
        assertTrue(warp.contains("fade = 1.0 - 0.35 * clamp(max(max(outside.x, outside.y), 0.0) / 0.08, 0.0, 1.0);"));
        assertTrue(warp.contains("vec2 source = clamp(1.0 - abs(1.0 - abs(p)), 0.0, 1.0);"));
        // A pixel whose shifted position lands on the HUD stays put rather than drawing a copy of the HUD
        int ghost = warp.indexOf("if (frameWarp.shift.z > 0.5 && texture(u_hud, source).r > 0.5) return uv;");
        assertTrue(ghost > masked && warp.indexOf("return source;") > ghost);
        for (String pass : new String[] {"planar_csc.frag", "sgsr.frag"}) {
            String source = read("src/main/jni/pyrowave-renderer/shaders/" + pass);
            assertTrue(pass, source.contains("= frameWarpUv(") && source.contains(", warpFade);"));
            assertTrue(pass, source.contains("#ifdef FRAME_WARP\n    rgb *= warpFade;\n#endif"));
        }
    }

    // The Vulkan video path follows the same rule: the mask where the pixel is, then the shifted position mirrored,
    // and a pixel whose shifted position lands on the HUD left in place
    @Test
    public void vulkanVideoWarpLeavesAPixelWhoseShiftedPositionIsHudInPlace() throws Exception {
        String warp = read("src/main/jni/vulkan/shaders/video_warp.frag");
        int masked = warp.indexOf("hud = texture(uHud, picture).r > 0.5;");
        int mirrored = warp.indexOf("p = 1.0 - abs(1.0 - abs(p));");
        int ghost = warp.indexOf("if (pc.warp.z <= 0.5 || texture(uHud, p).r <= 0.5) {");
        assertTrue(masked > 0 && mirrored > masked && ghost > mirrored);
        assertTrue(warp.indexOf("uv = pc.warpRect.xy + p * pc.warpRect.zw;") > ghost);
        assertTrue(warp.contains("outColor.rgb *= fade;"));
    }

    // Without Frame Warp nothing new runs: the plain pipelines, no set 1, no redraws
    @Test
    public void withoutFrameWarpThePlainPassesDraw() throws Exception {
        String cpp = read(NATIVE);
        assertTrue(cpp.contains("warp == nullptr ? upscalePipeline : warpUpscalePipeline"));
        assertTrue(cpp.contains("warp == nullptr ? pipeline : warpPipeline"));
        assertTrue(cpp.contains("if (schedule.held() >= 0 && warpRedraws() && warpWantsVsync()) {"));
        // Woken at a vsync only with a locked model, and then only while the view turns
        assertTrue(cpp.contains("return warp != nullptr && warpDrawn.active && (warpLookAfterPicture || warpDrawn.mayMove(warp));"));
        assertTrue(cpp.contains("if (!warpDrawn.mayMove(warp)) return true;"));
        // Only Frame Warp holds a picture for redraws
        assertTrue(cpp.contains("if (!warpRedraws()) return;\n            std::lock_guard<std::mutex> lock(slotMutex);\n" +
                "            schedule.hold(index);"));
    }
}

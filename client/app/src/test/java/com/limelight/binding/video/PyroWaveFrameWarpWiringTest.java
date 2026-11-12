package com.limelight.binding.video;

import org.junit.Test;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

import static org.junit.Assert.assertEquals;
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
        // From the picture's centre through the lens terms, which is uv less the shift for a flat picture
        int centred = warp.indexOf("vec2 q = uv - 0.5;");
        int turned = warp.indexOf("vec2 p = 0.5 + (q * frameWarp.lens.zw - frameWarp.shift.xy) / "
                + "max(1.0 + dot(q, frameWarp.lens.xy), 0.25);");
        assertTrue(centred > masked && turned > centred);
        assertFalse(warp.contains("uv - frameWarp.shift.xy;"));
        assertTrue(warp.indexOf("vec2 outside = max(-p, p - 1.0);") > turned);
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

    // The shift's uniform is the shift and then the lens terms, as the shader's block has them, and the renderer
    // writes both from what warp() gave it
    @Test
    public void theShiftUniformCarriesTheLensTerms() throws Exception {
        String warp = read("src/main/jni/pyrowave-renderer/shaders/frame_warp.glsl");
        Matcher block = Pattern.compile("uniform FrameWarpShift \\{(.*?)\\} frameWarp;", Pattern.DOTALL).matcher(warp);
        assertTrue(block.find());
        List<String> members = new ArrayList<>();
        Matcher member = Pattern.compile("(?m)^\\s*(vec4 \\w+);").matcher(block.group(1));
        while (member.find()) members.add(member.group(1));
        assertEquals(Arrays.asList("vec4 shift", "vec4 lens"), members);
        String cpp = read(NATIVE);
        assertTrue(cpp.contains("constexpr size_t WARP_SHIFT_FLOATS = 8;"));
        assertTrue(cpp.contains("const VkDescriptorBufferInfo shift = {slot.shift, 0, WARP_SHIFT_FLOATS * sizeof(float)};"));
        assertTrue(cpp.contains("createMappedBuffer(WARP_SHIFT_FLOATS * sizeof(float), VK_BUFFER_USAGE_UNIFORM_BUFFER_BIT,"));
        assertFalse(cpp.contains("4 * sizeof(float), VK_BUFFER_USAGE_UNIFORM_BUFFER_BIT"));
        int still = cpp.indexOf("if (code == FRAME_WARP_INACTIVE) frameWarpStill(out);");
        int shift = cpp.indexOf("slot.shiftMapped[0] = out[0];\n            slot.shiftMapped[1] = out[1];");
        int lens = cpp.indexOf("std::memcpy(slot.shiftMapped + 4, out + 4, 4 * sizeof(float));");
        assertTrue(still > 0 && shift > still && lens > shift);
    }

    // The Vulkan video path's push constants are one block in three places: both shaders and the renderer's struct.
    // The lens terms come last, and the block stays within the 128 bytes every Vulkan device takes.
    @Test
    public void vulkanVideoPushConstantsAgreeAndCarryTheLensTerms() throws Exception {
        Pattern blockPattern = Pattern.compile("layout\\(push_constant\\) uniform PushConstants \\{(.*?)\\} pc;",
                Pattern.DOTALL);
        List<String> expected = Arrays.asList("uvRect", "uvClamp", "params", "params2", "warp", "warpRect", "warpLens");
        for (String shader : new String[] {"video.vert", "video_common.glsl"}) {
            Matcher block = blockPattern.matcher(read("src/main/jni/vulkan/shaders/" + shader));
            assertTrue(shader, block.find());
            List<String> members = new ArrayList<>();
            Matcher member = Pattern.compile("(?m)^\\s*vec4 (\\w+);").matcher(block.group(1));
            while (member.find()) members.add(member.group(1));
            assertEquals(shader, expected, members);
        }
        String cpp = read("src/main/jni/vulkan/vulkan_renderer.cpp");
        int start = cpp.indexOf("struct PushConstants {");
        assertTrue(start > 0);
        List<String> fields = new ArrayList<>();
        Matcher field = Pattern.compile("(?m)^\\s*float (\\w+)\\[4\\];")
                .matcher(cpp.substring(start, cpp.indexOf("};", start)));
        while (field.find()) fields.add(field.group(1));
        assertEquals(expected, fields);
        assertTrue(expected.size() * 16 <= 128);
        assertTrue(cpp.contains("static_assert(sizeof(PushConstants) == 7 * 4 * sizeof(float) && sizeof(PushConstants) <= 128,"));
        assertTrue(cpp.contains("VkPushConstantRange range {VK_SHADER_STAGE_VERTEX_BIT | VK_SHADER_STAGE_FRAGMENT_BIT, 0, "
                + "sizeof(PushConstants)};"));
        // A picture drawn as it is has a flat picture's terms; otherwise what warp() gave
        int still = cpp.indexOf("if (warpCode == FRAME_WARP_INACTIVE) {\n            frameWarpStill(warpOut);");
        int lens = cpp.indexOf("memcpy(pc.warpLens, warpOut + 4, sizeof(pc.warpLens));");
        assertTrue(still > 0 && lens > still);
    }

    // Every renderer draws the mapping the shared core defines (nightjar_core's frame_warp::source): from the
    // picture's centre, through the lens terms, never closer than a quarter of the way to level with the camera.
    // Only the names differ.
    @Test
    public void allThreeRenderersTurnThePictureThroughTheSameLens() throws Exception {
        String core = read("../protocol/client/src/frame_warp.rs");
        assertTrue(core.contains("pub const FLAT: [f32; 4] = [0.0, 0.0, 1.0, 1.0];"));
        assertTrue(core.contains("pub const MIN_DEPTH: f32 = 0.25;"));
        assertTrue(core.contains("pub fn source(p: [f32; 2], shift: [f32; 2], lens: [f32; 4]) -> [f32; 2] {"));
        String[][] shaders = {
                {"src/main/jni/vulkan/shaders/video_warp.frag", "picture", "pc.warpLens", "pc.warp"},
                {"src/main/assets/shaders/warp.frag", "picture", "lens", "warp"},
                {"src/main/jni/pyrowave-renderer/shaders/frame_warp.glsl", "uv", "frameWarp.lens", "frameWarp.shift"},
        };
        for (String[] shader : shaders) {
            // Indentation aside
            String text = read(shader[0]).replaceAll("(?m)^ +", "");
            String mapping = "vec2 q = " + shader[1] + " - 0.5;\n"
                    + "vec2 p = 0.5 + (q * " + shader[2] + ".zw - " + shader[3] + ".xy) / max(1.0 + dot(q, "
                    + shader[2] + ".xy), 0.25);\n"
                    // The mirror and the darkening past an edge follow, from the turned position
                    + "vec2 outside = max(-p, p - 1.0);";
            assertTrue(shader[0], text.contains(mapping));
            // With no shift nothing is turned: the picture is drawn as it is
            assertTrue(shader[0], text.contains(shader[3] + ".x != 0.0 || " + shader[3] + ".y != 0.0")
                    || text.contains(shader[3] + ".x == 0.0 && " + shader[3] + ".y == 0.0) return uv;"));
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

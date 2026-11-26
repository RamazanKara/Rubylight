package com.limelight.binding.video;

import org.junit.Test;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import java.util.stream.Stream;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

/** PyroWave's renderer needs a GPU and a stream, so this pins its JNI exports and how Frame Warp reaches it. */
public class PyroWaveFrameWarpWiringTest {
    private static final String JAVA = "src/main/java/com/limelight/binding/video/PyroWaveDecoderRenderer.java";
    private static final String NATIVE_DIR = "src/main/jni/pyrowave-renderer";
    private static final String VULKAN_DIR = "src/main/jni/vulkan";

    private static String read(String path) throws Exception {
        return new String(Files.readAllBytes(Paths.get(path)), StandardCharsets.UTF_8).replace("\r\n", "\n");
    }

    private static String readSources(String directory, String prefix) throws Exception {
        StringBuilder joined = new StringBuilder();
        try (Stream<Path> files = Files.list(Paths.get(directory))) {
            List<Path> sorted = new ArrayList<>();
            files.filter(path -> {
                String name = path.getFileName().toString();
                return name.startsWith(prefix) && (name.endsWith(".cpp") || name.endsWith(".h"));
            }).sorted().forEach(sorted::add);
            for (Path path : sorted) {
                joined.append(read(path.toString())).append('\n');
            }
        }
        return joined.toString();
    }

    // A native without its export fails only when a stream first calls it
    @Test
    public void everyPyroWaveNativeHasItsExport() throws Exception {
        String java = read(JAVA);
        String cpp = readSources(NATIVE_DIR, "pyrowave_");
        List<String> natives = new ArrayList<>();
        Matcher declared = Pattern.compile("private static native \\w+(?:\\[\\])? (\\w+)\\(").matcher(java);
        while (declared.find()) natives.add(declared.group(1));
        assertTrue(natives.contains("nativeSetFrameWarp"));
        assertTrue(natives.contains("nativeSubmitRawFrame"));
        assertTrue(natives.contains("nativeSubmitChunksRaw"));
        for (String name : natives) {
            assertTrue(name, cpp.contains("Java_com_limelight_binding_video_PyroWaveDecoderRenderer_" + name + "("));
        }
        assertTrue(java.contains("private static native boolean nativeSetFrameWarp(long handle, long api);"));
        assertTrue(cpp.contains("PyroWaveDecoderRenderer_nativeSetFrameWarp(JNIEnv *, jclass, jlong handle,\n" +
                "                                                                          jlong api) {"));
        assertTrue(cpp.contains("if (frameWarp) renderer->noteFrameNumber(uint32_t(frameNumber));"));
    }

    // The renderer is made before the stream says it offers Frame Warp: it takes it once the decoder is set
    // up, and each picture's number reaches it folded into the submit call (zero extra JNI crossing)
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
        assertTrue(java.contains("nativeSubmitRawFrame(handle, rawPtr, length, ptsUs, frameNumber, frameWarp,"));
        assertTrue(java.contains("nativeSubmitDirectFrame(handle, directData, length, ptsUs, frameNumber, frameWarp,"));
        assertTrue(java.contains("nativeSubmitFrame(handle, data, length, ptsUs, frameNumber, frameWarp,"));
    }

    // Both passes take the shift the same way: no mask read without a shift or where the mask does not apply, the
    // picture taken back inside and faded past its edges, and a pixel whose shifted position is HUD probing back
    // along the shift before it stays in place (as the Vulkan, GLES and desktop presenters)
    @Test
    public void bothPassesShiftFillAndProbeAlike() throws Exception {
        String warp = read("src/main/jni/pyrowave-renderer/shaders/frame_warp.glsl");
        int zero = warp.indexOf("if (frameWarp.shift.x == 0.0 && frameWarp.shift.y == 0.0) return uv;");
        int masked = warp.indexOf("if (frameWarp.shift.z > 0.5) {\n        if (texture(u_hud, uv).r > 0.5) return uv;");
        assertTrue(zero > 0 && masked > zero);
        // From the picture's centre through the lens terms, which is uv less the shift for a flat picture
        int centred = warp.indexOf("vec2 q = uv - 0.5;");
        int turned = warp.indexOf("vec2 p = 0.5 + (q * frameWarp.lens.zw - frameWarp.shift.xy) / "
                + "max(1.0 + dot(q, frameWarp.lens.xy), 0.25);");
        assertTrue(centred > masked && turned > centred);
        assertFalse(warp.contains("uv - frameWarp.shift.xy;"));
        int filled = warp.indexOf("vec2 source = frameWarpEdgeV2(p, frameWarp.v2.x, edgeFade);");
        assertTrue(filled > turned);
        assertFalse("the old mirror at the edge is gone", warp.contains("abs(1.0 - abs(p))"));
        // A pixel whose shifted position lands on the HUD probes back along the shift, and without the scene there
        // stays put rather than drawing a copy of the HUD
        int ghost = warp.indexOf("if (frameWarp.shift.z > 0.5 && texture(u_hud, source).r > 0.5) "
                + "return frameWarpProbe(uv, p, fade);");
        assertTrue(ghost > filled && warp.indexOf("fade = edgeFade;\n    return source;") > ghost);
        int probe = warp.indexOf("vec2 frameWarpProbe(vec2 uv, vec2 p, out float fade) {");
        assertTrue(probe > 0 && warp.indexOf("    }\n    return uv;\n}", probe) > probe);
        for (String pass : new String[] {"planar_csc.frag", "sgsr.frag"}) {
            String source = read("src/main/jni/pyrowave-renderer/shaders/" + pass);
            assertTrue(pass, source.contains("= frameWarpUv(") && source.contains(", warpFade);"));
            assertTrue(pass, source.contains("#ifdef FRAME_WARP\n    rgb *= warpFade;\n#endif"));
        }
    }

    // The shift's uniform is the shift, the lens terms and the row, as the shader's block has them, and the renderer
    // writes all three from what warp() gave it
    @Test
    public void theShiftUniformCarriesTheLensTermsAndTheRow() throws Exception {
        String warp = read("src/main/jni/pyrowave-renderer/shaders/frame_warp.glsl");
        Matcher block = Pattern.compile("uniform FrameWarpShift \\{(.*?)\\} frameWarp;", Pattern.DOTALL).matcher(warp);
        assertTrue(block.find());
        List<String> members = new ArrayList<>();
        Matcher member = Pattern.compile("(?m)^\\s*(vec4 \\w+);").matcher(block.group(1));
        while (member.find()) members.add(member.group(1));
        assertEquals(Arrays.asList("vec4 shift", "vec4 lens", "vec4 v2"), members);
        String cpp = readSources(NATIVE_DIR, "pyrowave_");
        assertTrue(cpp.contains("constexpr size_t WARP_SHIFT_FLOATS = 12;"));
        assertTrue(cpp.contains("const VkDescriptorBufferInfo shift = {slot.shift, 0, WARP_SHIFT_FLOATS * sizeof(float)};"));
        assertTrue(cpp.contains("createMappedBuffer(WARP_SHIFT_FLOATS * sizeof(float), VK_BUFFER_USAGE_UNIFORM_BUFFER_BIT,"));
        assertFalse(cpp.contains("4 * sizeof(float), VK_BUFFER_USAGE_UNIFORM_BUFFER_BIT"));
        int still = cpp.indexOf("if (code == FRAME_WARP_INACTIVE) frameWarpStill(out);");
        int shift = cpp.indexOf("slot.shiftMapped[0] = out[0];\n    slot.shiftMapped[1] = out[1];");
        int lens = cpp.indexOf("std::memcpy(slot.shiftMapped + 4, out + 4, 4 * sizeof(float));");
        int row = cpp.indexOf("frameWarpV2Row(out, slot.shiftMapped + 8);");
        assertTrue(still > 0 && shift > still && lens > shift && row > lens);
    }

    // The Vulkan video path's push constants are one block in three places: both shaders and the renderer's struct.
    // The lens terms and the row come last, and the block stays within the 128 bytes every Vulkan device takes.
    @Test
    public void vulkanVideoPushConstantsAgreeAndCarryTheLensTermsAndTheRow() throws Exception {
        Pattern blockPattern = Pattern.compile("layout\\(push_constant\\) uniform PushConstants \\{(.*?)\\} pc;",
                Pattern.DOTALL);
        List<String> expected = Arrays.asList("uvRect", "uvClamp", "params", "params2", "warp", "warpRect", "warpLens",
                "warpV2");
        for (String shader : new String[] {"video.vert", "video_common.glsl"}) {
            Matcher block = blockPattern.matcher(read("src/main/jni/vulkan/shaders/" + shader));
            assertTrue(shader, block.find());
            List<String> members = new ArrayList<>();
            Matcher member = Pattern.compile("(?m)^\\s*vec4 (\\w+);").matcher(block.group(1));
            while (member.find()) members.add(member.group(1));
            assertEquals(shader, expected, members);
        }
        String cpp = readSources(VULKAN_DIR, "vulkan_renderer");
        int start = cpp.indexOf("struct PushConstants {");
        assertTrue(start > 0);
        List<String> fields = new ArrayList<>();
        Matcher field = Pattern.compile("(?m)^\\s*float (\\w+)\\[4\\];")
                .matcher(cpp.substring(start, cpp.indexOf("};", start)));
        while (field.find()) fields.add(field.group(1));
        assertEquals(expected, fields);
        assertTrue(expected.size() * 16 <= 128);
        assertTrue(cpp.contains("static_assert(sizeof(PushConstants) == 8 * 4 * sizeof(float) && sizeof(PushConstants) <= 128,"));
        assertTrue(cpp.contains("VkPushConstantRange range {VK_SHADER_STAGE_VERTEX_BIT | VK_SHADER_STAGE_FRAGMENT_BIT, 0, "
                + "sizeof(PushConstants)};"));
        // A picture drawn as it is has a flat picture's terms and no row; otherwise what warp() gave
        int still = cpp.indexOf("if (warpCode == FRAME_WARP_INACTIVE) {\n            frameWarpStill(warpOut);");
        int lens = cpp.indexOf("memcpy(pc.warpLens, warpOut + 4, sizeof(pc.warpLens));");
        int row = cpp.indexOf("frameWarpV2Row(warpOut, pc.warpV2);");
        assertTrue(still > 0 && lens > still && row > lens);
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
                {"src/main/jni/vulkan/shaders/video_warp.frag", "picture", "pc.warpLens", "pc.warp", "pc.warpV2"},
                {"src/main/assets/shaders/warp.frag", "picture", "lens", "warp", "uWarpV2"},
                {"src/main/jni/pyrowave-renderer/shaders/frame_warp.glsl", "uv", "frameWarp.lens", "frameWarp.shift",
                        "frameWarp.v2"},
        };
        for (String[] shader : shaders) {
            // Indentation aside
            String text = read(shader[0]).replaceAll("(?m)^ +", "");
            String mapping = "vec2 q = " + shader[1] + " - 0.5;\n"
                    + "vec2 p = 0.5 + (q * " + shader[2] + ".zw - " + shader[3] + ".xy) / max(1.0 + dot(q, "
                    + shader[2] + ".xy), 0.25);\n"
                    // The edge fill follows, from the turned position, as strong as the row says
                    + "float edgeFade;\n";
            assertTrue(shader[0], text.contains(mapping));
            assertTrue(shader[0], text.indexOf(" = frameWarpEdgeV2(p, " + shader[4] + ".x, edgeFade);")
                    > text.indexOf(mapping));
            // With no shift nothing is turned: the picture is drawn as it is
            assertTrue(shader[0], text.contains(shader[3] + ".x != 0.0 || " + shader[3] + ".y != 0.0")
                    || text.contains(shader[3] + ".x == 0.0 && " + shader[3] + ".y == 0.0) return uv;"));
        }
    }

    // The Vulkan video path follows the same rule: the mask where the pixel is, then the shifted position filled,
    // and a pixel whose shifted position lands on the HUD probing back along the shift before it stays in place
    @Test
    public void vulkanVideoWarpProbesBeforeLeavingAPixelInPlace() throws Exception {
        String warp = read("src/main/jni/vulkan/shaders/video_warp.frag");
        int masked = warp.indexOf("hud = texture(uHud, picture).r > 0.5;");
        int filled = warp.indexOf("vec2 shifted = frameWarpEdgeV2(p, pc.warpV2.x, edgeFade);");
        int ghost = warp.indexOf("bool found = pc.warp.z <= 0.5 || texture(uHud, shifted).r <= 0.5;");
        int probe = warp.indexOf("found = frameWarpProbe(p, shifted, edgeFade);");
        assertTrue(masked > 0 && filled > masked && ghost > filled && probe > ghost);
        assertTrue(warp.indexOf("uv = pc.warpRect.xy + shifted * pc.warpRect.zw;") > probe);
        assertTrue(warp.contains("outColor.rgb *= fade;"));
    }

    // The edge fill and the HUD probe are the shared core's (nightjar_core's frame_warp_v2::edge_v2 and hud_probe):
    // the same function in all three shaders, with the core's reflection, ramp and longest probe, and nothing left
    // of the old mirror at the edge
    @Test
    public void allThreeRenderersFillAndProbeAsTheCoreDoes() throws Exception {
        String core = read("../protocol/client/src/frame_warp_v2.rs");
        assertTrue(core.contains("pub const EDGE_V2_REFLECT: f32 = 0.25;"));
        assertTrue(core.contains("pub fn edge_v2(p: [f32; 2], strength: f32) -> ([f32; 2], f32) {"));
        assertTrue(core.contains("pub fn hud_probe("));
        assertTrue(read("../protocol/client/src/frame_warp.rs").contains("pub const EDGE_RAMP: f32 = 0.08;"));
        assertTrue(read("../protocol/core/src/frame_warp_v2.rs").contains("pub const MAX_HUD_GUARD: u8 = 8;"));
        String fill = "vec2 frameWarpEdgeV2(vec2 p, float strength, out float fade) {\n"
                + "vec2 outside = max(-p, p - 1.0);\n"
                + "fade = 1.0 - clamp(strength, 0.0, 1.0) * smoothstep(0.0, 0.08, max(outside.x, outside.y));\n"
                + "vec2 inside = mix(p, mix(-p * 0.25, 1.0 - (p - 1.0) * 0.25, greaterThan(p, vec2(1.0))),\n"
                + "greaterThan(outside, vec2(0.0)));\n"
                + "return clamp(inside, 0.0, 1.0);\n"
                + "}";
        String[][] shaders = {
                {"src/main/jni/vulkan/shaders/video_warp.frag", "pc.warp", "pc.warpV2", "uHud"},
                {"src/main/assets/shaders/warp.frag", "warp", "uWarpV2", "hudMask"},
                {"src/main/jni/pyrowave-renderer/shaders/frame_warp.glsl", "frameWarp.shift", "frameWarp.v2", "u_hud"},
        };
        for (String[] shader : shaders) {
            String text = read(shader[0]).replaceAll("(?m)^ +", "");
            assertTrue(shader[0], text.contains(fill));
            // One mask cell along the shift's direction, never past the core's longest probe or the row's reach
            assertTrue(shader[0], text.contains("vec2 cell = normalize(" + shader[1] + ".xy) / vec2(textureSize("
                    + shader[3] + ", 0));"));
            assertTrue(shader[0], text.contains("for (int k = 1; k <= 8; ++k) {\nif (float(k) > " + shader[2]
                    + ".y) break;"));
            assertTrue(shader[0], text.contains("frameWarpEdgeV2(p - float(k) * cell, " + shader[2] + ".x, "));
            assertFalse(shader[0], text.contains("abs(1.0 - abs(p))"));
            assertFalse(shader[0], text.contains("0.35"));
        }
    }

    // Without Frame Warp nothing new runs: the plain pipelines, no set 1, no redraws
    @Test
    public void withoutFrameWarpThePlainPassesDraw() throws Exception {
        String cpp = readSources(NATIVE_DIR, "pyrowave_");
        assertTrue(cpp.contains("warp == nullptr ? upscalePipeline : warpUpscalePipeline"));
        assertTrue(cpp.contains("warp == nullptr ? pipeline : warpPipeline"));
        assertTrue(cpp.contains("if (schedule.held() >= 0 && warpRedraws() && warpWantsVsync()) {"));
        // Woken at a vsync only with a locked model, and then only while the view turns
        assertTrue(cpp.contains("return warp != nullptr && warpDrawn.active && (warpLookAfterPicture || warpDrawn.mayMove(warp));"));
        assertTrue(cpp.contains("if (!warpDrawn.mayMove(warp)) return true;"));
        // Only Frame Warp holds a picture for redraws
        assertTrue(cpp.contains("if (!warpRedraws()) return;\n    std::lock_guard<std::mutex> lock(slotMutex);\n" +
                "    schedule.hold(index);"));
    }
}

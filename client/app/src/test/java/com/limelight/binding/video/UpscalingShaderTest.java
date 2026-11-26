package com.limelight.binding.video;

import org.junit.Test;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.regex.Pattern;
import static org.junit.Assert.*;

public class UpscalingShaderTest {
    private String source(String name) throws Exception {
        return Files.readString(Path.of("src/main/assets/shaders", name)).replace("\r\n", "\n");
    }

    @Test
    public void shadersUseTheGles30InterfaceAndBalancedSyntax() throws Exception {
        for (String name : new String[] {"upscale.vert", "bilinear.frag", "easu.frag", "rcas.frag", "sgsr.frag",
                "warp.frag"}) {
            String text = source(name);
            assertTrue(name, text.startsWith("#version 300 es\n"));
            assertTrue(name, text.contains("precision highp float;"));
            assertEquals(name, 1, occurrences(text, "void main()"));
            text = text.replaceAll("(?m)//.*$", "");
            for (String pair : new String[] {"{}", "()", "[]"}) {
                int depth = 0;
                for (char c : text.toCharArray()) {
                    if (c == pair.charAt(0)) depth++;
                    if (c == pair.charAt(1)) depth--;
                    assertTrue(name, depth >= 0);
                }
                assertEquals(name, 0, depth);
            }
            assertFalse(name, text.contains("#include"));
            assertFalse(name, text.contains("textureGather"));
            assertFalse(name, text.contains("gl_FragColor"));
            if (name.endsWith(".frag")) assertTrue(name, text.contains("out vec4 color;"));
        }
        for (String name : new String[] {"bilinear.frag", "easu.frag", "sgsr.frag", "warp.frag"}) {
            assertTrue(source(name).contains("#extension GL_OES_EGL_image_external_essl3 : require"));
            assertTrue(source(name).contains("samplerExternalOES source;"));
            assertTrue(source(name).contains("uniform mat4 textureTransform;"));
        }
    }

    @Test
    public void frameWarpReadsTheMaskWhereThePixelIsThenFillsAndProbes() throws Exception {
        String warp = source("warp.frag");
        assertTrue(warp.contains("uniform vec3 warp;"));
        assertTrue(warp.contains("uniform vec4 lens;"));
        assertTrue(warp.contains("uniform vec4 uWarpV2;"));
        assertTrue(warp.contains("uniform lowp sampler2D hudMask;"));
        // No shift reads no mask; the mask only when it applies.
        assertTrue(warp.contains("if (warp.x != 0.0 || warp.y != 0.0) {"));
        assertTrue(warp.contains("if (warp.z > 0.5) {"));
        // The mask is in picture space with rows from the top; upscale.vert's uv has y up.
        assertTrue(warp.contains("vec2 picture = vec2(uv.x, 1.0 - uv.y);"));
        assertTrue(warp.contains("texture(hudMask, picture)"));
        // Sampled where the turn brings the pixel from: from the picture's centre through the lens terms, which is
        // the position less the shift for a flat picture; then taken back inside past the edges and faded with the
        // distance past them, as the PC's row says (the same rule as the Vulkan, PyroWave and desktop presenters).
        int centred = warp.indexOf("vec2 q = picture - 0.5;");
        int turned = warp.indexOf("vec2 p = 0.5 + (q * lens.zw - warp.xy) / max(1.0 + dot(q, lens.xy), 0.25);");
        assertTrue(centred > 0 && turned > centred);
        assertFalse(warp.contains("picture - warp.xy;"));
        int filled = warp.indexOf("vec2 shifted = frameWarpEdgeV2(p, uWarpV2.x, edgeFade);");
        assertTrue(filled > turned);
        assertFalse("the old mirror at the edge is gone", warp.contains("abs(1.0 - abs(p))"));
        // A pixel whose shifted position lands on the HUD looks back along the shift for the scene behind it, and
        // without it stays where it is rather than drawing a copy of the HUD.
        int ghost = warp.indexOf("bool found = warp.z <= 0.5 || texture(hudMask, shifted).r <= 0.5;");
        int probe = warp.indexOf("found = frameWarpProbe(p, shifted, edgeFade);");
        assertTrue(ghost > filled && probe > ghost);
        assertTrue(warp.indexOf("fade = edgeFade;", probe) > probe);
        assertTrue(warp.indexOf("at = vec2(shifted.x, 1.0 - shifted.y);") > probe);
        assertTrue(warp.contains("textureTransform * vec4(at, 0.0, 1.0)"));
        assertTrue(warp.contains(".rgb * fade, 1.0)"));
    }

    // The row reaches warp.frag as the PC gave it, through GlesFrameWarpV2, and a picture drawn as it is has none.
    @Test
    public void frameWarpSetsTheRowFromTheLastFourFloats() throws Exception {
        String gles = Files.readString(Path.of("src/main/java/com/limelight/binding/video/GlesUpscaler.java"))
                .replace("\r\n", "\n");
        int lens = gles.indexOf("active ? warpOut[6] : 1f, active ? warpOut[7] : 1f);");
        assertTrue(lens > 0 && gles.indexOf("warpV2.apply(active, warpOut);") > lens);
        assertTrue(gles.contains("warpV2.locate(upscaleProgram);"));
        float[] out = {0.1f, 0.2f, 3f, 4f, 0f, 0f, 1f, 1f, 0.35f, 2f, 0.75f, 33f};
        assertEquals(0.35f, GlesFrameWarpV2.value(true, out, 0), 0f);
        assertEquals(2f, GlesFrameWarpV2.value(true, out, 1), 0f);
        assertEquals(33f, GlesFrameWarpV2.value(true, out, 3), 0f);
        assertEquals("drawn as it is", 0f, GlesFrameWarpV2.value(false, out, 0), 0f);
        assertEquals("an array without the row", 0f, GlesFrameWarpV2.value(true, new float[8], 1), 0f);
        assertEquals(com.limelight.nvstream.jni.MoonBridge.FRAME_WARP_FLOATS,
                GlesFrameWarpV2.FIRST + GlesFrameWarpV2.FLOATS);
    }

    // The lens terms reach warp.frag as the PC gave them (y down, as the shader's picture coordinates), and a
    // picture drawn as it is has a flat picture's.
    @Test
    public void frameWarpSetsTheLensTermsWithTheShift() throws Exception {
        String gles = Files.readString(Path.of("src/main/java/com/limelight/binding/video/GlesUpscaler.java"))
                .replace("\r\n", "\n");
        assertTrue(gles.contains("lensUniform = GLES30.glGetUniformLocation(upscaleProgram, \"lens\");"));
        assertTrue(gles.contains("GLES30.glUniform4f(lensUniform, 0f, 0f, 1f, 1f);"));
        int shift = gles.indexOf("GLES30.glUniform3f(warpUniform, active ? warpOut[0] : 0f, active ? warpOut[1] : 0f,");
        int lens = gles.indexOf("GLES30.glUniform4f(lensUniform, active ? warpOut[4] : 0f, active ? warpOut[5] : 0f,\n"
                + "                active ? warpOut[6] : 1f, active ? warpOut[7] : 1f);");
        assertTrue(shift > 0 && lens > shift);
        // Redraws between pictures still go by the shift in pixels
        assertTrue(gles.contains("drawnShiftX = active ? warpOut[2] : 0f;"));
        assertTrue(gles.contains("drawnShiftY = active ? warpOut[3] : 0f;"));
    }

    @Test
    public void sgsrRetainsTheMobileKernelAndPackagedBsdNotice() throws Exception {
        String sgsr = source("sgsr.frag");
        assertEquals(12, occurrences(sgsr, "weightY(pl.x"));
        assertEquals(13, occurrences(sgsr, "loadGreen("));
        assertTrue(sgsr.contains("uniform vec4 viewportInfo;"));
        assertTrue(sgsr.contains("uniform float edgeSharpness;"));
        assertTrue(sgsr.contains("edgeVote > 8.0 / 255.0"));
        assertTrue(sgsr.contains("2.181818 / max(sum, 1.0e-8)"));
        assertTrue(sgsr.contains("clamp(edgeSharpness * finalY, minY, maxY)"));
        assertTrue(sgsr.contains("-23.0 / 255.0, 23.0 / 255.0"));
        assertTrue(sgsr.contains("textureTransform * vec4(p, 0.0, 1.0)"));
        String notice = Files.readString(Path.of("src/main/assets/NOTICE-SGSR1.txt"));
        assertTrue(notice.contains("Copyright (c) 2025, Qualcomm Innovation Center, Inc."));
        assertTrue(notice.contains("Copyright (c) 2023, Qualcomm Innovation Center, Inc."));
        assertTrue(notice.contains("Redistributions in binary form"));
        assertTrue(notice.contains("Neither the name of the copyright holder"));
        assertTrue(notice.contains("THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS"));
        assertTrue(notice.contains("BSD-3-Clause"));
        assertTrue(notice.contains("d926f074bcb9d714e179f1ce0fcb9ee2eeb5074e"));
    }

    @Test
    public void easuAndRcasContainTheAmdKernelsAndFiniteFlatColorHandling() throws Exception {
        String easu = source("easu.frag");
        assertEquals(12, occurrences(easu, "FsrEasuTapF(aC, aW,"));
        assertEquals(4, occurrences(easu, "FsrEasuSetF(dir, len,"));
        for (int i = 0; i < 4; i++) assertTrue(easu.contains("uniform vec4 con" + i + ";"));
        assertTrue(easu.contains("25.0 / 16.0"));
        assertTrue(easu.contains("clamp(aC / aW, min4, max4)"));
        assertTrue(easu.contains("max(reversal, vec2(1.0e-8))"));
        String rcas = source("rcas.frag");
        assertEquals(6, occurrences(rcas, "FsrRcasLoadF(")); // Definition and five taps.
        assertTrue(rcas.contains("0.25 - 1.0 / 16.0"));
        assertTrue(rcas.contains("uniform float sharpness;"));
        assertTrue(rcas.contains("max(4.0 * mx4, vec3(1.0e-8))"));
        assertTrue(rcas.contains("min(4.0 * mn4 - 4.0, vec3(-1.0e-8))"));
        String notice = Files.readString(Path.of("src/main/assets/NOTICE-FSR1.txt"));
        assertTrue(notice.contains("Copyright (c) 2021 Advanced Micro Devices"));
        assertTrue(notice.contains("Permission is hereby granted, free of charge"));
        assertTrue(notice.contains("a21ffb8f6c13233ba336352bdff293894c706575"));
    }

    private int occurrences(String text, String token) {
        return (int) Pattern.compile(Pattern.quote(token)).matcher(text).results().count();
    }
}

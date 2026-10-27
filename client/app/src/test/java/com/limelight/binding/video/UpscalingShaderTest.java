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
        for (String name : new String[] {"upscale.vert", "bilinear.frag", "easu.frag", "rcas.frag", "sgsr.frag"}) {
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
        for (String name : new String[] {"bilinear.frag", "easu.frag", "sgsr.frag"}) {
            assertTrue(source(name).contains("#extension GL_OES_EGL_image_external_essl3 : require"));
            assertTrue(source(name).contains("samplerExternalOES source;"));
            assertTrue(source(name).contains("uniform mat4 textureTransform;"));
        }
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

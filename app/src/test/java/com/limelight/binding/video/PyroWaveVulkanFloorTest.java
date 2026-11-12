package com.limelight.binding.video;

import org.junit.Test;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.MessageDigest;
import java.util.HexFormat;
import java.util.List;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import java.util.stream.Collectors;
import java.util.stream.Stream;
import static org.junit.Assert.*;

public class PyroWaveVulkanFloorTest {
    private static final Path RENDERER = Path.of("src/main/jni/pyrowave-renderer");

    private static String text(Path path) throws Exception {
        return Files.readString(path).replace("\r\n", "\n");
    }

    // A Vulkan 1.1 driver refuses SPIR-V newer than 1.3.
    @Test
    public void rendererShadersLoadOnVulkan11() throws Exception {
        Matcher shader = Pattern.compile("uint32_t (\\w+_spv)\\[\\] = \\{0x07230203,0x([0-9a-f]{8})")
                .matcher(text(RENDERER.resolve("shaders_spv.h")));
        int shaders = 0;
        while (shader.find()) {
            shaders++;
            assertTrue(shader.group(1), Integer.parseUnsignedInt(shader.group(2), 16) <= 0x00010300);
        }
        // The fullscreen vertex shader and the plain and sharpened fragments, each
        // also with Frame Warp.
        assertEquals(5, shaders);
    }

    @Test
    public void everyLanguageNamesTheSameVulkanVersion() throws Exception {
        List<Path> strings;
        try (Stream<Path> dirs = Files.list(Path.of("src/main/res"))) {
            strings = dirs.filter(dir -> dir.getFileName().toString().startsWith("values"))
                    .map(dir -> dir.resolve("strings.xml")).filter(Files::exists).collect(Collectors.toList());
        }
        int found = 0;
        Pattern entry = Pattern.compile("<string name=\"pyrowave_needs_vulkan\">([^<]*)</string>");
        for (Path file : strings) {
            Matcher match = entry.matcher(text(file));
            if (!match.find()) continue;
            found++;
            assertTrue(file.toString(), match.group(1).contains("1.1"));
            assertFalse(file.toString(), match.group(1).contains("1.3"));
        }
        assertTrue(found >= 2);
    }

    // The bundled libraries are the ones the rebuild notes describe, at the pins the notices name.
    @Test
    public void prebuiltLibrariesMatchTheRebuildNotes() throws Exception {
        String parity = text(Path.of("../internal/BUTTERPOLLO_PARITY.md"));
        for (String abi : new String[] {"arm64-v8a", "x86_64"}) {
            Matcher row = Pattern.compile("\\| " + abi + " \\| [\\d,]+ \\| `([0-9a-f]{64})` \\|").matcher(parity);
            String last = null;
            while (row.find()) last = row.group(1);
            byte[] library = Files.readAllBytes(RENDERER.resolve("prebuilt/" + abi + "/libpyrowave-shared.so"));
            assertEquals(abi, last, HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(library)));
        }
        Matcher pins = Pattern.compile("Pinned dependency sources: PyroWave `([0-9a-f]{40})`, Granite\\s+`([0-9a-f]{40})`").matcher(parity);
        assertTrue(pins.find());
        String notices = text(Path.of("src/main/assets/pyrowave_notices.txt"));
        assertTrue(notices.contains("PyroWave " + pins.group(1) + " (MIT)"));
        assertTrue(notices.contains("Granite " + pins.group(2) + " (MIT)"));
    }
}

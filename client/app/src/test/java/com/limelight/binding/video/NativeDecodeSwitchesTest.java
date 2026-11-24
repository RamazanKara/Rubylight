package com.limelight.binding.video;

import org.junit.Test;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.List;
import java.util.stream.Stream;

import static org.junit.Assert.assertTrue;

/** The native decode switches need a GPU and a stream, so this pins their defaults and wiring. */
public class NativeDecodeSwitchesTest {
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

    // The GPU waits for the decoder's frame through a sync fd: measured better on the Retroid, so
    // it is on unless the property is 0, never only when it is 1.
    @Test
    public void theVulkanGpuWaitForTheDecoderIsOnUnlessThePropertyIs0() throws Exception {
        String source = readSources("src/main/jni/vulkan", "vulkan_renderer");
        int get = source.indexOf("__system_property_get(\"debug.nightjar.vk_sync_fd\", syncFdWait);");
        assertTrue(get > 0);
        String after = source.substring(get, source.indexOf('{', get) + 1);
        assertTrue(after, after.contains("if (strcmp(syncFdWait, \"0\") != 0) {"));
    }

    // "PyroWave early screen image": the setting reaches the native renderer, and the property
    // wins over it when it is 1 or 0.
    @Test
    public void thePyroWavePreacquireSettingReachesTheRendererAndThePropertyOverridesIt() throws Exception {
        String renderer = read("src/main/java/com/limelight/binding/video/MediaCodecDecoderRenderer.java");
        assertTrue(renderer.contains("prefs.upscalingMode, prefs.upscalingSharpness, context, prefs.gpuMaxClocks,\n" +
                "                    prefs.pyroWavePreacquire)"));
        String pyroWave = read("src/main/java/com/limelight/binding/video/PyroWaveDecoderRenderer.java");
        assertTrue(pyroWave.contains("Context context, boolean maxClocks, boolean preacquire) {"));
        assertTrue(pyroWave.contains("SgsrConstants.edgeSharpness(sharpness), preacquire,"));
        assertTrue(pyroWave.contains("float edgeSharpness, boolean preacquire,"));
        String nativeSource = readSources("src/main/jni/pyrowave-renderer", "pyrowave_");
        assertTrue(nativeSource.contains("jfloat edgeSharpness, jboolean preacquire,"));
        assertTrue(nativeSource.contains("upscale, edgeSharpness, preacquire)) {"));
        assertTrue(nativeSource.contains("configurePreacquire(preacquireSetting);"));
        assertTrue(nativeSource.contains("__system_property_get(\"debug.nightjar.pyro_preacquire\", value);"));
        assertTrue(nativeSource.contains(
                "const bool requested = std::strcmp(value, \"1\") == 0 || (std::strcmp(value, \"0\") != 0 && setting);"));
    }
}

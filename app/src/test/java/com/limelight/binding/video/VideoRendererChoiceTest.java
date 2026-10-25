package com.limelight.binding.video;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import com.limelight.binding.video.UpscalingPolicy.Mode;
import com.limelight.binding.video.VideoRendererChoice.Reason;
import com.limelight.binding.video.VideoRendererChoice.Renderer;

import org.junit.Test;

public class VideoRendererChoiceTest {
    private static VideoRendererChoice choose(boolean vulkan, boolean pyroWave, int sdk, boolean supported,
                                              boolean packed, Mode mode) {
        return VideoRendererChoice.choose(vulkan, pyroWave, sdk, () -> supported, packed, mode);
    }

    @Test
    public void vulkanOffKeepsTheDirectPathAndNeverProbes() {
        VideoRendererChoice choice = VideoRendererChoice.choose(false, false, 34, () -> {
            throw new AssertionError("must not probe when Vulkan is off");
        }, false, Mode.OFF);
        assertEquals(Renderer.DIRECT, choice.renderer);
        assertEquals(Reason.NONE, choice.reason);
        assertFalse(choice.upscalingOverridden);
    }

    @Test
    public void vulkanOffWithUpscalingUsesGles() {
        VideoRendererChoice choice = choose(false, false, 34, true, false, Mode.FSR1);
        assertEquals(Renderer.GLES, choice.renderer);
        assertEquals(Reason.NONE, choice.reason);
    }

    @Test
    public void vulkanOnWhenSupported() {
        VideoRendererChoice choice = choose(true, false, 29, true, false, Mode.OFF);
        assertEquals(Renderer.VULKAN, choice.renderer);
        assertEquals(Reason.NONE, choice.reason);
        assertFalse(choice.upscalingOverridden);
    }

    @Test
    public void pyroWaveKeepsItsOwnRenderer() {
        VideoRendererChoice choice = choose(true, true, 34, true, false, Mode.OFF);
        assertEquals(Renderer.DIRECT, choice.renderer);
        assertEquals(Reason.PYROWAVE, choice.reason);
    }

    @Test
    public void androidNineAndOlderCannotUseVulkan() {
        VideoRendererChoice choice = choose(true, false, 28, true, false, Mode.OFF);
        assertEquals(Renderer.DIRECT, choice.renderer);
        assertEquals(Reason.API_LEVEL, choice.reason);
    }

    @Test
    public void unsupportedDeviceFallsBackWithAReason() {
        VideoRendererChoice choice = choose(true, false, 34, false, false, Mode.OFF);
        assertEquals(Renderer.DIRECT, choice.renderer);
        assertEquals(Reason.UNSUPPORTED, choice.reason);
    }

    @Test
    public void unsupportedDeviceWithUpscalingStillGetsTheUpscaler() {
        VideoRendererChoice choice = choose(true, false, 34, false, false, Mode.SGSR1);
        assertEquals(Renderer.GLES, choice.renderer);
        assertEquals(Reason.UNSUPPORTED, choice.reason);
    }

    @Test
    public void packedFullChromaUsesGlesAndSaysSo() {
        VideoRendererChoice choice = choose(true, false, 34, true, true, Mode.OFF);
        assertEquals(Renderer.GLES, choice.renderer);
        assertEquals(Reason.PACKED_444, choice.reason);
    }

    @Test
    public void vulkanWinsOverUpscalingAndFlagsIt() {
        VideoRendererChoice choice = choose(true, false, 34, true, false, Mode.FSR1);
        assertEquals(Renderer.VULKAN, choice.renderer);
        assertEquals(Reason.NONE, choice.reason);
        assertTrue(choice.upscalingOverridden);
    }

    @Test
    public void probeOnlyRunsOnceNothingElseRulesVulkanOut() {
        boolean[] probed = {false};
        VideoRendererChoice.choose(true, true, 34, () -> probed[0] = true, false, Mode.OFF);
        VideoRendererChoice.choose(true, false, 28, () -> probed[0] = true, false, Mode.OFF);
        VideoRendererChoice.choose(true, false, 34, () -> probed[0] = true, true, Mode.OFF);
        assertFalse(probed[0]);
    }
}

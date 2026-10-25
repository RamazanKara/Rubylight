package com.limelight.binding.video;

import java.util.function.BooleanSupplier;

/**
 * Which renderer presents a MediaCodec stream. Pure, so each rule can be unit tested.
 *
 * With "Render through Vulkan" off nothing changes: the direct SurfaceView path, or the GLES
 * presenter when an upscaler is selected. With it on, Vulkan is used unless something rules it
 * out, and then the {@link Reason} says what, so the user is told instead of silently getting a
 * different renderer.
 */
public final class VideoRendererChoice {
    public enum Renderer {
        VULKAN,
        /** The GLES presenter may be used (upscaling, or packed 4:4:4); it can still decline later. */
        GLES,
        /** The decoder renders straight into the SurfaceView. */
        DIRECT
    }

    /** Why Vulkan, though asked for, is not the renderer. NONE when it is, or was not asked for. */
    public enum Reason {
        NONE, PYROWAVE, API_LEVEL, UNSUPPORTED, PACKED_444
    }

    public static final int MIN_SDK = 29;

    public final Renderer renderer;
    public final Reason reason;
    /** Vulkan is used although an upscaler was selected; the upscaler is not applied. */
    public final boolean upscalingOverridden;

    private VideoRendererChoice(Renderer renderer, Reason reason, boolean upscalingOverridden) {
        this.renderer = renderer;
        this.reason = reason;
        this.upscalingOverridden = upscalingOverridden;
    }

    /**
     * @param vulkanSupported asked only once nothing else rules Vulkan out, because probing loads
     *                        the native library and creates a Vulkan instance
     */
    public static VideoRendererChoice choose(boolean vulkanPreferred, boolean pyroWave, int sdkInt,
                                             BooleanSupplier vulkanSupported, boolean packed444,
                                             UpscalingPolicy.Mode upscaling) {
        boolean upscale = upscaling != UpscalingPolicy.Mode.OFF;
        Renderer without = pyroWave ? Renderer.DIRECT : (upscale || packed444 ? Renderer.GLES : Renderer.DIRECT);
        if (!vulkanPreferred) return new VideoRendererChoice(without, Reason.NONE, false);
        if (pyroWave) return new VideoRendererChoice(without, Reason.PYROWAVE, false);
        if (packed444) return new VideoRendererChoice(without, Reason.PACKED_444, false);
        if (sdkInt < MIN_SDK) return new VideoRendererChoice(without, Reason.API_LEVEL, false);
        if (!vulkanSupported.getAsBoolean()) return new VideoRendererChoice(without, Reason.UNSUPPORTED, false);
        return new VideoRendererChoice(Renderer.VULKAN, Reason.NONE, upscale);
    }
}

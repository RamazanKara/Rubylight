package com.limelight.binding.video;

import org.junit.Test;

import static org.junit.Assert.*;

public class GlesUpscalerCapabilityTest {
    @Test
    public void tenBitOutputNeedsTheBt2020PqExtensionAndATenBitColourBuffer() {
        String extensions = "EGL_KHR_image EGL_EXT_gl_colorspace_bt2020_pq EGL_ANDROID_presentation_time";
        assertTrue(GlesUpscaler.tenBitOutputSupported(extensions, true));
        assertFalse(GlesUpscaler.tenBitOutputSupported(extensions, false));
        assertFalse(GlesUpscaler.tenBitOutputSupported("EGL_KHR_image", true));
        assertFalse(GlesUpscaler.tenBitOutputSupported(null, true));
    }
}

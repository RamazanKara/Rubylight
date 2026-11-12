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

    @Test
    public void frameWarpDrawsThePictureAgainOnceItsShiftMovedAQuarterPixel() {
        assertFalse(GlesUpscaler.needsRedraw(10f, -4f, 10f, -4f));
        assertFalse(GlesUpscaler.needsRedraw(10f, -4f, 10.1f, -4.1f));
        assertTrue(GlesUpscaler.needsRedraw(10f, -4f, 10.25f, -4f));
        assertTrue(GlesUpscaler.needsRedraw(10f, -4f, 10.2f, -4.2f));
        assertTrue("a shift that ended is drawn away", GlesUpscaler.needsRedraw(3f, 0f, 0f, 0f));
    }

    @Test
    public void frameWarpLooksAgainOnlyWhileTheModelIsLockedAndSomethingMoves() {
        long deflected = com.limelight.nvstream.jni.MoonBridge.FRAME_WARP_STICK_DEFLECTED;
        assertFalse("unlocked: nothing to redraw, whatever is sent", GlesUpscaler.mayMove(false, 0f, 0f, 4, 9 | deflected));
        assertFalse("locked and still", GlesUpscaler.mayMove(true, 0f, 0f, 4, 4));
        assertTrue("input sent since the draw", GlesUpscaler.mayMove(true, 0f, 0f, 4, 5));
        assertTrue("a deflected stick turns without new input", GlesUpscaler.mayMove(true, 0f, 0f, 4, 4 | deflected));
        assertTrue("a shift on screen can still move", GlesUpscaler.mayMove(true, 0f, -2.5f, 4, 4));
        // The count is the low 32 bits, and wraps
        assertFalse(GlesUpscaler.mayMove(true, 0f, 0f, -1, 0xFFFF_FFFFL));
        assertTrue(GlesUpscaler.mayMove(true, 0f, 0f, -1, 0L));
    }
}

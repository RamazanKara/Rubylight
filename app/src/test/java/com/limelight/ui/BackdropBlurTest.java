package com.limelight.ui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.graphics.Bitmap;

import androidx.core.graphics.ColorUtils;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;

/** The backdrop is only allowed if the screen's text stays readable over the brightest cover there is. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = 30, application = Application.class)
public class BackdropBlurTest {
    private static final int CANVAS = 0xFF0B0D12;
    /** On-surface, on-surface-variant (night palette) and the dimmer supporting text. */
    private static final int[] TEXT = {0xFFE3E4EE, 0xFFC1C4D2, 0xFFA9ADBD};
    /** The bar and the focus line sit in the top of the window; the posters start below. */
    private static final float TEXT_ZONE = 0.30f;

    private static Bitmap solid(int color) {
        Bitmap bitmap = Bitmap.createBitmap(300, 450, Bitmap.Config.ARGB_8888);
        bitmap.eraseColor(color);
        return bitmap;
    }

    @Test
    public void textKeepsFourAndAHalfToOneOverAWhiteCover() {
        Bitmap made = BackdropBlur.make(solid(0xFFFFFFFF), CANVAS);
        for (int y = 0; y < BackdropBlur.HEIGHT; y++) {
            if ((y + 0.5f) / BackdropBlur.HEIGHT > TEXT_ZONE) break;
            for (int x = 0; x < BackdropBlur.WIDTH; x++) {
                for (int text : TEXT) {
                    double ratio = ColorUtils.calculateContrast(text, made.getPixel(x, y));
                    assertTrue(String.format("%08X at %d,%d: %.2f:1", text, x, y, ratio), ratio >= 4.5);
                }
            }
        }
    }

    @Test
    public void aWhiteCoverStillShowsThroughBelowTheTextZone() {
        Bitmap made = BackdropBlur.make(solid(0xFFFFFFFF), CANVAS);
        int middle = made.getPixel(BackdropBlur.WIDTH / 2, BackdropBlur.HEIGHT / 2);
        int top = made.getPixel(BackdropBlur.WIDTH / 2, 0);
        assertTrue("the middle is the art, not the canvas", ColorUtils.calculateLuminance(middle) > 5 * ColorUtils.calculateLuminance(top));
    }

    @Test
    public void theThumbnailIsSmallOpaqueAndTheSameForTheSameCover() {
        Bitmap cover = solid(0xFF3366CC);
        Bitmap made = BackdropBlur.make(cover, CANVAS);
        assertEquals(BackdropBlur.WIDTH, made.getWidth());
        assertEquals(BackdropBlur.HEIGHT, made.getHeight());
        assertFalse(made.hasAlpha());
        assertTrue(made.sameAs(BackdropBlur.make(cover, CANVAS)));
    }

    @Test
    public void aGameWithNoArtIsThePlainCanvas() {
        Bitmap made = BackdropBlur.make(null, CANVAS);
        for (int y = 0; y < BackdropBlur.HEIGHT; y += 9) assertEquals(CANVAS, made.getPixel(40, y));
    }

    @Test
    public void theFootFadesTowardTheCanvasSoTheScreenHasNoHardEdge() {
        Bitmap made = BackdropBlur.make(solid(0xFFFFFFFF), CANVAS);
        int foot = made.getPixel(10, BackdropBlur.HEIGHT - 1);
        int middle = made.getPixel(10, BackdropBlur.HEIGHT / 2);
        assertTrue(ColorUtils.calculateLuminance(foot) < ColorUtils.calculateLuminance(middle));
    }
}

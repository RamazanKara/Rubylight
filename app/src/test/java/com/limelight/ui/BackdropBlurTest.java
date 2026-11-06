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
    /** The design's label and secondary label. */
    private static final int[] TEXT = {0xFFF5F6FB, 0xFFCDD0DC};
    /** Day's label and secondary label. */
    private static final int[] DAY_TEXT = {0xFF1A1C22, 0xFF3D404D};

    private static Bitmap solid(int color) {
        Bitmap bitmap = Bitmap.createBitmap(300, 450, Bitmap.Config.ARGB_8888);
        bitmap.eraseColor(color);
        return bitmap;
    }

    @Test
    public void textKeepsFourAndAHalfToOneOverAWhiteCoverEverywhere() {
        Bitmap made = BackdropBlur.make(solid(0xFFFFFFFF), CANVAS);
        for (int y = 0; y < BackdropBlur.HEIGHT; y += 3) {
            for (int x = 0; x < BackdropBlur.WIDTH; x += 5) {
                for (int text : TEXT) {
                    double ratio = ColorUtils.calculateContrast(text, made.getPixel(x, y));
                    assertTrue(String.format("%08X at %d,%d: %.2f:1", text, x, y, ratio), ratio >= 4.5);
                }
            }
        }
    }

    @Test
    public void whiteIsHeldToASafeGreyAndDarkArtKeepsItsColour() {
        int white = BackdropBlur.bakeNight(0xFFFFFFFF, new float[3]);
        assertEquals(82, (white >> 8) & 0xFF, 2);
        int dark = BackdropBlur.bakeNight(0xFF102040, new float[3]);
        assertTrue("dark art is not held down", (dark & 0xFF) >= 0x40);
    }

    @Test
    public void yellowTurnsTowardOrangeAndOtherHuesStay() {
        assertEquals(22f, BackdropBlur.yellowToOrange(60f), 0.01f);
        assertEquals(200f, BackdropBlur.yellowToOrange(200f), 0.01f);
        assertEquals(10f, BackdropBlur.yellowToOrange(10f), 0.01f);
        // No jump at the band's edge.
        assertEquals(26f, BackdropBlur.yellowToOrange(26f), 0.5f);
    }

    @Test
    public void dayLiftsBlackToASoftGreyAndDarkTextStaysReadable() {
        Bitmap made = BackdropBlur.make(solid(0xFF000000), 0xFFF6F7FB, true, true);
        assertEquals(BackdropBlur.HEIGHT, made.getWidth());
        assertEquals(BackdropBlur.WIDTH, made.getHeight());
        int pixel = made.getPixel(20, 40);
        assertEquals(188, (pixel >> 8) & 0xFF, 2);
        for (int text : DAY_TEXT) assertTrue(ColorUtils.calculateContrast(text, pixel) >= 4.5);
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
}

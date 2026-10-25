package com.limelight.ui;

import static org.junit.Assert.*;

import android.app.Application;
import android.graphics.Bitmap;
import android.graphics.Color;
import android.widget.FrameLayout;
import android.widget.ImageView;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.annotation.Config;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class ArtFitTest {
    @Test
    public void portraitArtIsCroppedAndEverythingElseIsShownWhole() {
        assertTrue("2:3 poster", !ArtFit.letterboxes(600, 900));
        assertTrue("3:4 box art", !ArtFit.letterboxes(600, 800));
        assertTrue("exactly 1.25", !ArtFit.letterboxes(400, 500));
        assertTrue("just under 1.25", ArtFit.letterboxes(400, 499));
        assertTrue("square", ArtFit.letterboxes(600, 600));
        assertTrue("Steam header", ArtFit.letterboxes(460, 215));
    }

    @Test
    public void aPlaceholderOrNothingIsNotArt() {
        assertFalse(ArtFit.letterboxes(1, 1));
        assertFalse(ArtFit.letterboxes(0, 0));
        assertFalse(ArtFit.letterboxes(-1, 10));
    }

    private static Bitmap solid(int width, int height, int color) {
        Bitmap bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888);
        bitmap.eraseColor(color);
        return bitmap;
    }

    @Test
    public void aPosterViewCropsPortraitArtAndLetterboxesALandscapeCover() {
        try (var controller = Robolectric.buildActivity(MotionTest.Screen.class).setup().visible()) {
            FrameLayout frame = new FrameLayout(controller.get());
            PosterImageView image = new PosterImageView(controller.get());
            frame.addView(image);
            android.graphics.drawable.Drawable surface = frame.getBackground();

            image.setImageBitmap(solid(300, 400, Color.RED));
            assertEquals(ImageView.ScaleType.CENTER_CROP, image.getScaleType());
            assertSame("portrait art keeps the frame's surface", surface, frame.getBackground());

            image.setImageBitmap(solid(460, 215, Color.GREEN));
            assertEquals(ImageView.ScaleType.FIT_CENTER, image.getScaleType());
            assertNotNull("the blurred copy fills the poster behind the cover", frame.getBackground());
            assertNotSame(surface, frame.getBackground());

            image.setImageBitmap(solid(300, 300, Color.BLUE));
            assertEquals("a square is shown whole too", ImageView.ScaleType.FIT_CENTER, image.getScaleType());

            // A recycled card shows a placeholder, then portrait art: no letterbox is left over.
            image.setImageDrawable(new android.graphics.drawable.BitmapDrawable(image.getResources(), solid(1, 1, Color.TRANSPARENT)));
            assertEquals(ImageView.ScaleType.CENTER_CROP, image.getScaleType());
            assertFalse(frame.getBackground().getClass().getSimpleName().equals("Backdrop"));
            image.setImageBitmap(solid(600, 900, Color.RED));
            assertEquals(ImageView.ScaleType.CENTER_CROP, image.getScaleType());
        }
    }

    @Test
    public void theLetterboxBackdropIsTheArtBlurredAndDarker() {
        Bitmap art = Bitmap.createBitmap(460, 215, Bitmap.Config.ARGB_8888);
        art.eraseColor(Color.WHITE);
        Bitmap blurred = ArtFit.blurred(art);
        assertTrue(blurred.getWidth() <= 16 && blurred.getHeight() <= 24);
        assertEquals(Color.WHITE, blurred.getPixel(2, 2));

        try (var controller = Robolectric.buildActivity(MotionTest.Screen.class).setup().visible()) {
            FrameLayout frame = new FrameLayout(controller.get());
            PosterImageView image = new PosterImageView(controller.get());
            frame.addView(image);
            image.setImageBitmap(art);
            android.graphics.drawable.Drawable backdrop = frame.getBackground();
            backdrop.setBounds(0, 0, 40, 60);
            Bitmap drawn = Bitmap.createBitmap(40, 60, Bitmap.Config.ARGB_8888);
            backdrop.draw(new android.graphics.Canvas(drawn));
            int pixel = drawn.getPixel(20, 30);
            // White art under the 0.55 darkening: light grey at most, so text over it keeps its contrast.
            assertTrue("red " + Color.red(pixel), Color.red(pixel) <= 0x90 && Color.red(pixel) >= 0x80);
            assertEquals(255, Color.alpha(pixel));
        }
    }
}

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
    public void onlyArtCloseToAPosterIsCroppedAndEverythingElseIsShownWhole() {
        assertTrue("2:3 poster", !ArtFit.letterboxes(600, 900));
        assertTrue("1.45", !ArtFit.letterboxes(400, 580));
        assertTrue("1.57", !ArtFit.letterboxes(400, 628));
        assertTrue("1.6", ArtFit.letterboxes(400, 640));
        assertTrue("1.4", ArtFit.letterboxes(400, 560));
        assertTrue("3:4 box art keeps its edges", ArtFit.letterboxes(600, 800));
        assertTrue("1:2", ArtFit.letterboxes(300, 600));
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
    public void aPosterViewCropsPosterArtAndLetterboxesEverythingElseOverTheBackdropItIsGiven() {
        try (var controller = Robolectric.buildActivity(MotionTest.Screen.class).setup().visible()) {
            FrameLayout frame = new FrameLayout(controller.get());
            PosterImageView image = new PosterImageView(controller.get());
            frame.addView(image);
            android.graphics.drawable.Drawable surface = frame.getBackground();

            Bitmap poster = solid(300, 450, Color.RED);
            assertNull("a poster needs no backdrop", ArtFit.backdropFor(poster));
            image.setArt(poster, null);
            assertEquals(ImageView.ScaleType.CENTER_CROP, image.getScaleType());
            assertSame("poster art keeps the frame's surface", surface, frame.getBackground());

            Bitmap banner = solid(460, 215, Color.GREEN);
            Bitmap backdrop = ArtFit.backdropFor(banner);
            assertNotNull(backdrop);
            image.setArt(banner, backdrop);
            assertEquals(ImageView.ScaleType.FIT_CENTER, image.getScaleType());
            assertNotNull("the blurred copy fills the poster behind the cover", frame.getBackground());
            assertNotSame(surface, frame.getBackground());
            assertSame("the backdrop made on the loader's thread is the one on screen", backdrop,
                    ((android.graphics.drawable.BitmapDrawable) frame.getBackground()).getBitmap());

            image.setArt(solid(300, 400, Color.BLUE), null);
            assertEquals("3:4 is shown whole too", ImageView.ScaleType.FIT_CENTER, image.getScaleType());
            assertNotNull("a cover without a backdrop sits on the frame's own surface", frame.getBackground());
            assertFalse("the last cover's backdrop is gone", frame.getBackground() instanceof android.graphics.drawable.BitmapDrawable);

            // Binding does no bitmap work: a plain bitmap, a resource or a placeholder never builds a backdrop.
            image.setImageBitmap(solid(460, 215, Color.GREEN));
            assertEquals(ImageView.ScaleType.FIT_CENTER, image.getScaleType());
            assertFalse(frame.getBackground() instanceof android.graphics.drawable.BitmapDrawable);

            // A recycled card shows a placeholder, then poster art: no letterbox is left over.
            image.setImageDrawable(new android.graphics.drawable.BitmapDrawable(image.getResources(), solid(1, 1, Color.TRANSPARENT)));
            assertEquals(ImageView.ScaleType.CENTER_CROP, image.getScaleType());
            image.setArt(poster, null);
            assertEquals(ImageView.ScaleType.CENTER_CROP, image.getScaleType());
        }
    }

    @Test
    public void theBackdropIsTakenFromThePosterShapedMiddleOfTheArt() {
        // A banner is cropped to the 2:3 its poster shows, in the middle, before it is blurred.
        assertArrayEquals(new int[] {316, 0, 287, 430}, ArtFit.posterCrop(920, 430));
        assertArrayEquals(new int[] {0, 0, 600, 900}, ArtFit.posterCrop(600, 900));
        assertArrayEquals("3:4 loses its sides", new int[] {33, 0, 533, 800}, ArtFit.posterCrop(600, 800));
        assertArrayEquals(new int[] {0, 50, 600, 900}, ArtFit.posterCrop(600, 1000));
        assertArrayEquals(new int[] {0, 150, 600, 900}, ArtFit.posterCrop(600, 1200));
        assertArrayEquals("a square", new int[] {100, 0, 400, 600}, ArtFit.posterCrop(600, 600));
    }

    @Test
    public void theBackdropIsATinyCopyOfTheArtMadeInStepsAndDarkerThanIt() {
        // Left third red, a white middle third (a logo), right third blue: high contrast, not a solid colour.
        Bitmap art = Bitmap.createBitmap(920, 430, Bitmap.Config.ARGB_8888);
        for (int x = 0; x < 920; x++) {
            int color = x < 307 ? Color.RED : x < 613 ? Color.WHITE : Color.BLUE;
            for (int y = 0; y < 430; y += 43) art.setPixel(x, y, color);
        }
        Bitmap blurred = ArtFit.blurred(art);
        assertEquals(8, blurred.getWidth());
        assertEquals(12, blurred.getHeight());

        Bitmap solid = Bitmap.createBitmap(460, 215, Bitmap.Config.ARGB_8888);
        solid.eraseColor(Color.WHITE);
        assertEquals(Color.WHITE, ArtFit.blurred(solid).getPixel(2, 2));

        try (var controller = Robolectric.buildActivity(MotionTest.Screen.class).setup().visible()) {
            FrameLayout frame = new FrameLayout(controller.get());
            PosterImageView image = new PosterImageView(controller.get());
            frame.addView(image);
            image.setArt(art, ArtFit.backdropFor(art));
            android.graphics.drawable.Drawable backdrop = frame.getBackground();
            // The test runtime does not rasterize, so the darkening is checked as the filter the poster draws with.
            assertTrue(backdrop instanceof android.graphics.drawable.BitmapDrawable);
            Bitmap tiny = ((android.graphics.drawable.BitmapDrawable) backdrop).getBitmap();
            assertTrue("the backdrop is a tiny copy, not a blur pass", tiny.getWidth() <= 16 && tiny.getHeight() <= 24);
            assertTrue("darkened so text over it keeps its contrast",
                    backdrop.getColorFilter() instanceof android.graphics.LightingColorFilter);
            android.graphics.LightingColorFilter filter = (android.graphics.LightingColorFilter) backdrop.getColorFilter();
            assertTrue("a light touch, not a mud", (filter.getColorMultiply() & 0xFF) >= 0xA0);
        }
    }

    @Test
    public void aHardwareBitmapIsNeverReadForABackdrop() {
        Bitmap art = Bitmap.createBitmap(460, 215, Bitmap.Config.ARGB_8888).copy(Bitmap.Config.HARDWARE, false);
        // The loader only builds backdrops from the software bitmap it decoded; a hardware one gets none, not an exception.
        if (art != null && art.getConfig() == Bitmap.Config.HARDWARE) assertNull(ArtFit.backdropFor(art));
    }
}

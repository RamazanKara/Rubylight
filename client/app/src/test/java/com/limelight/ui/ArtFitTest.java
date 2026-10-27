package com.limelight.ui;

import static org.junit.Assert.*;

import android.app.Application;
import android.graphics.Bitmap;
import android.graphics.Color;
import android.graphics.Rect;
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
    public void aCoverInGreyHasGreyBandsWhetherTheFilterOrTheArtComesFirst() {
        try (var controller = Robolectric.buildActivity(MotionTest.Screen.class).setup().visible()) {
            FrameLayout frame = new FrameLayout(controller.get());
            PosterImageView image = new PosterImageView(controller.get());
            frame.addView(image);
            android.graphics.ColorFilter grey = new android.graphics.ColorMatrixColorFilter(new android.graphics.ColorMatrix());
            Bitmap banner = solid(460, 215, Color.GREEN);

            // A card is bound (filter set) before its cover has loaded.
            image.setColorFilter(grey);
            image.setArt(banner, ArtFit.backdropFor(banner));
            assertSame(grey, frame.getBackground().getColorFilter());

            // Bound again for an installed game: the bands take their colour back.
            image.setColorFilter(null);
            assertNull(frame.getBackground().getColorFilter());
        }
    }

    @Test
    public void aPosterViewCropsPosterArtAndLetterboxesEverythingElseOverTheBandsItIsGiven() {
        try (var controller = Robolectric.buildActivity(MotionTest.Screen.class).setup().visible()) {
            FrameLayout frame = new FrameLayout(controller.get());
            PosterImageView image = new PosterImageView(controller.get());
            frame.addView(image);
            android.graphics.drawable.Drawable surface = frame.getBackground();

            Bitmap poster = solid(300, 450, Color.RED);
            assertNull("a poster needs no bands", ArtFit.backdropFor(poster));
            image.setArt(poster, null);
            assertEquals(ImageView.ScaleType.CENTER_CROP, image.getScaleType());
            assertSame("poster art keeps the frame's surface", surface, frame.getBackground());

            Bitmap banner = solid(460, 215, Color.GREEN);
            ArtFit.Bands bands = ArtFit.backdropFor(banner);
            assertNotNull(bands);
            image.setArt(banner, bands);
            assertEquals(ImageView.ScaleType.FIT_CENTER, image.getScaleType());
            assertNotSame(surface, frame.getBackground());
            assertTrue("the gradient fills the poster behind the cover", frame.getBackground() instanceof ArtFit.Backdrop);
            assertSame("the bands made on the loader's thread are the ones on screen", bands,
                    ((ArtFit.Backdrop) frame.getBackground()).bands());

            image.setArt(solid(300, 400, Color.BLUE), null);
            assertEquals("3:4 is shown whole too", ImageView.ScaleType.FIT_CENTER, image.getScaleType());
            assertNotNull("a cover without bands sits on the frame's own surface", frame.getBackground());
            assertFalse("the last cover's bands are gone", frame.getBackground() instanceof ArtFit.Backdrop);

            // Binding does no bitmap work: a plain bitmap, a resource or a placeholder never builds bands.
            image.setImageBitmap(solid(460, 215, Color.GREEN));
            assertEquals(ImageView.ScaleType.FIT_CENTER, image.getScaleType());
            assertFalse(frame.getBackground() instanceof ArtFit.Backdrop);

            // A recycled card shows a placeholder, then poster art: no letterbox is left over.
            image.setArt(banner, bands);
            image.setImageDrawable(new android.graphics.drawable.BitmapDrawable(image.getResources(), solid(1, 1, Color.TRANSPARENT)));
            assertEquals(ImageView.ScaleType.CENTER_CROP, image.getScaleType());
            assertFalse("a placeholder drops the bands", frame.getBackground() instanceof ArtFit.Backdrop);
            image.setArt(poster, null);
            assertEquals(ImageView.ScaleType.CENTER_CROP, image.getScaleType());
        }
    }

    @Test
    public void theBandColoursAreTheAverageOfTheArtsFourEdgeRowsDarkenedALittle() {
        // High contrast, not a solid colour: the top rows are half red and half blue, then green below them;
        // the bottom rows are white after a black row. Only the four edge rows count.
        Bitmap art = Bitmap.createBitmap(920, 430, Bitmap.Config.ARGB_8888);
        art.eraseColor(Color.GREEN);
        for (int y = 0; y < 4; y++) {
            for (int x = 0; x < 920; x++) art.setPixel(x, y, x < 460 ? Color.RED : Color.BLUE);
        }
        for (int x = 0; x < 920; x++) art.setPixel(x, 425, Color.BLACK);
        for (int y = 426; y < 430; y++) {
            for (int x = 0; x < 920; x++) art.setPixel(x, y, Color.WHITE);
        }
        ArtFit.Bands bands = ArtFit.backdropFor(art);
        assertNotNull(bands);
        assertEquals("half red, half blue, then 75 per cent", Color.rgb(95, 0, 95), bands.top);
        assertEquals("white, then 75 per cent", Color.rgb(191, 191, 191), bands.bottom);
        assertEquals(430f / 920f, bands.aspect, 0.0001f);

        // A solid cover gives its own colour, darkened: nothing is lost to a blur.
        ArtFit.Bands green = ArtFit.backdropFor(solid(460, 215, Color.rgb(0, 200, 0)));
        assertEquals(Color.rgb(0, 150, 0), green.top);
        assertEquals(Color.rgb(0, 150, 0), green.bottom);
    }

    @Test
    public void aCoverShorterThanFourRowsStillGivesColours() {
        ArtFit.Bands bands = ArtFit.backdropFor(solid(120, 20, Color.WHITE));
        assertNotNull(bands);
        assertEquals(Color.rgb(191, 191, 191), bands.top);
    }

    @Test
    public void theGradientBandsAreAsTallAsTheLetterboxTheArtLeavesAndNeverNegative() {
        ArtFit.Backdrop banner = new ArtFit.Backdrop(ArtFit.backdropFor(solid(460, 215, Color.WHITE)));
        Rect poster = new Rect(0, 0, 300, 450);
        // The banner is 140 high in a 450 high poster: 155 above and below, 0.344 of the height each.
        assertEquals(155f / 450f, banner.band(poster), 0.001f);
        banner.setBounds(poster);
        // The test runtime does not rasterize; this checks the gradient draws without a shader error.
        banner.draw(new android.graphics.Canvas(Bitmap.createBitmap(300, 450, Bitmap.Config.ARGB_8888)));

        ArtFit.Backdrop box = new ArtFit.Backdrop(ArtFit.backdropFor(solid(600, 800, Color.WHITE)));
        assertEquals("3:4 leaves a thin band", (450f - 400f) / 2f / 450f, box.band(poster), 0.001f);

        ArtFit.Backdrop tall = new ArtFit.Backdrop(ArtFit.backdropFor(solid(300, 600, Color.WHITE)));
        assertEquals("art taller than the poster has no top or bottom band", 0f, tall.band(poster), 0.0001f);
        tall.setBounds(poster);
        tall.draw(new android.graphics.Canvas(Bitmap.createBitmap(300, 450, Bitmap.Config.ARGB_8888)));
        assertEquals(android.graphics.PixelFormat.OPAQUE, banner.getOpacity());
    }

    @Test
    public void aHardwareBitmapIsNeverReadForBands() {
        Bitmap art = Bitmap.createBitmap(460, 215, Bitmap.Config.ARGB_8888).copy(Bitmap.Config.HARDWARE, false);
        // The loader only builds bands from the software bitmap it decoded; a hardware one gets none, not an exception.
        if (art != null && art.getConfig() == Bitmap.Config.HARDWARE) assertNull(ArtFit.backdropFor(art));
    }

    @Test
    public void aBandFadesToADarkerShadeOfItsOwnEdgeColourNeverToTheCardSurface() {
        // Dark art in a light theme: the card surface is near white and must not show in the bands.
        ArtFit.Backdrop dark = new ArtFit.Backdrop(ArtFit.backdropFor(solid(460, 215, Color.rgb(40, 60, 100))));
        int[] stops = dark.stops();
        assertEquals(Color.rgb(30, 45, 75), stops[1]);
        assertEquals("the top edge is the top colour, darker", Color.rgb(17, 25, 41), stops[0]);
        assertEquals(Color.rgb(17, 25, 41), stops[3]);
        for (int stop : stops) {
            assertTrue("no stop is lighter than the art's edge colour", Color.red(stop) <= 30 && Color.green(stop) <= 45
                    && Color.blue(stop) <= 75);
        }
        assertTrue("the poster edge is darker than where the art starts", Color.blue(stops[0]) < Color.blue(stops[1]));
    }
}

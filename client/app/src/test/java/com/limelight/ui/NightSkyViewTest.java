package com.limelight.ui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertNotNull;

import android.app.Application;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.view.View;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;

/** The PCs page's sky: the moon stands where the design has it and keeps clear of a status bar, and the baked glass gets a picture. */
@RunWith(RobolectricTestRunner.class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = 30, application = Application.class)
public class NightSkyViewTest {
    private static final int W = 412;
    private static final int H = 915;

    private static Bitmap drawn(NightSkyView sky) {
        sky.measure(View.MeasureSpec.makeMeasureSpec(W, View.MeasureSpec.EXACTLY), View.MeasureSpec.makeMeasureSpec(H, View.MeasureSpec.EXACTLY));
        sky.layout(0, 0, W, H);
        Bitmap out = Bitmap.createBitmap(W, H, Bitmap.Config.ARGB_8888);
        sky.draw(new Canvas(out));
        return out;
    }

    private static boolean amber(int color) {
        return ((color >> 16) & 0xFF) > 200 && (color & 0xFF) < 150;
    }

    @Test
    @Config(qualifiers = "w412dp-h915dp-mdpi")
    public void onAPhoneTheMoonFollowsTheSafeArea() {
        try (var controller = Robolectric.buildActivity(MotionTest.Screen.class).setup()) {
            NightSkyView sky = new NightSkyView(controller.get());
            int x = Math.round(W * .563f);
            sky.setSafeTop(24);
            Bitmap under = drawn(sky);
            assertEquals("the moon's middle, 60dp under the safe top", true, amber(under.getPixel(x, 84)));
            sky.setSafeTop(100);
            Bitmap lower = drawn(sky);
            assertEquals("the moon went down with it", false, amber(lower.getPixel(x, 84)));
            assertEquals(true, amber(lower.getPixel(x, 160)));
        }
    }

    /** "Einstellungen" ran into the moon in the middle of the page: Settings has it at the end, clear of a long title. */
    @Test
    @Config(qualifiers = "w412dp-h915dp-mdpi")
    public void aPageWithABigTitleHasItsMoonAtTheEnd() {
        try (var controller = Robolectric.buildActivity(MotionTest.Screen.class).setup()) {
            NightSkyView sky = new NightSkyView(controller.get());
            sky.setSafeTop(24);
            sky.setMoonAtEnd();
            Bitmap at = drawn(sky);
            assertEquals(true, amber(at.getPixel(Math.round(W * .767f), 71)));
            assertEquals("the title's room is free", false, amber(at.getPixel(Math.round(W * .6f), 71)));
        }
    }

    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-mdpi")
    public void onATvOfFiveHundredAndFortyDpTheWholeMoonIsOnScreenAboveTheCards() {
        try (var controller = Robolectric.buildActivity(MotionTest.Screen.class).setup()) {
            NightSkyView sky = new NightSkyView(controller.get());
            sky.measure(View.MeasureSpec.makeMeasureSpec(960, View.MeasureSpec.EXACTLY), View.MeasureSpec.makeMeasureSpec(540, View.MeasureSpec.EXACTLY));
            sky.layout(0, 0, 960, 540);
            Bitmap out = Bitmap.createBitmap(960, 540, Bitmap.Config.ARGB_8888);
            sky.draw(new Canvas(out));
            int x = Math.round(960 * .8125f);
            assertEquals(true, amber(out.getPixel(x, 112)));
            // 150 + 86 scaled by 540/720 ends at 177dp; the cards start at 196.
            assertEquals(false, amber(out.getPixel(x, 190)));
        }
    }

    @Test
    public void theBakedGlassGetsASmallPicture() {
        try (var controller = Robolectric.buildActivity(MotionTest.Screen.class).setup()) {
            NightSkyView sky = new NightSkyView(controller.get());
            Bitmap baked = sky.bake(W, H);
            assertNotNull(baked);
            assertNotEquals(W, baked.getWidth());
        }
    }
}

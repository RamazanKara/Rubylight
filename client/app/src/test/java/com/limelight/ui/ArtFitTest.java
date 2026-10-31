package com.limelight.ui;

import static org.junit.Assert.*;

import android.app.Activity;
import android.app.Application;
import android.graphics.Bitmap;
import android.graphics.Color;
import android.graphics.Matrix;
import android.graphics.RectF;
import android.view.LayoutInflater;
import android.view.View;
import android.widget.FrameLayout;
import android.widget.ImageView;

import com.limelight.R;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.annotation.Config;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class ArtFitTest {
    private static Bitmap solid(int width, int height) {
        Bitmap bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888);
        bitmap.eraseColor(Color.GREEN);
        return bitmap;
    }

    /** Where the art is drawn inside a 200 x 300 poster frame. */
    private static RectF drawn(PosterImageView image, Bitmap art) {
        image.setArt(art);
        image.measure(View.MeasureSpec.makeMeasureSpec(200, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(300, View.MeasureSpec.EXACTLY));
        image.layout(0, 0, 200, 300);
        RectF bounds = new RectF(0, 0, art.getWidth(), art.getHeight());
        Matrix matrix = new Matrix(image.getImageMatrix());
        matrix.mapRect(bounds);
        return bounds;
    }

    @Test
    public void everyCoverShapeIsCroppedToFillItsFrameWithNoBandsAndNoStretch() {
        try (var controller = Robolectric.buildActivity(MotionTest.Screen.class).setup().visible()) {
            FrameLayout frame = new FrameLayout(controller.get());
            PosterImageView image = new PosterImageView(controller.get());
            frame.addView(image);
            int[][] shapes = {{600, 800}, {460, 215}, {600, 600}, {300, 600}, {600, 900}};
            String[] names = {"3:4 box", "Steam banner", "square", "1:2", "2:3 poster"};
            for (int i = 0; i < shapes.length; i++) {
                Bitmap art = solid(shapes[i][0], shapes[i][1]);
                RectF drawn = drawn(image, art);
                assertEquals(names[i], ImageView.ScaleType.CENTER_CROP, image.getScaleType());
                assertTrue(names[i] + " reaches every edge of the frame: " + drawn,
                        drawn.left <= 0.5f && drawn.top <= 0.5f && drawn.right >= 199.5f && drawn.bottom >= 299.5f);
                assertEquals(names[i] + " is centred", 100f, drawn.centerX(), 0.5f);
                assertEquals(names[i] + " is centred", 150f, drawn.centerY(), 0.5f);
                assertEquals(names[i] + " keeps its own shape", (float) shapes[i][1] / shapes[i][0],
                        drawn.height() / drawn.width(), 0.01f);
            }
        }
    }

    @Test
    public void aCardThatShowedAnIconCropsItsNextCover() {
        try (var controller = Robolectric.buildActivity(MotionTest.Screen.class).setup().visible()) {
            PosterImageView image = new PosterImageView(controller.get());
            image.setScaleType(ImageView.ScaleType.CENTER_INSIDE);
            ArtFit.reset(image);
            assertEquals(ImageView.ScaleType.CENTER_CROP, image.getScaleType());
            image.setScaleType(ImageView.ScaleType.FIT_CENTER);
            image.setArt(solid(460, 215));
            assertEquals(ImageView.ScaleType.CENTER_CROP, image.getScaleType());
        }
    }

    private static void assertCoverFrame(Activity activity, int layout, int id) {
        View root = LayoutInflater.from(activity).inflate(layout, new FrameLayout(activity), false);
        View cover = root.findViewById(id);
        String name = activity.getResources().getResourceEntryName(layout);
        assertTrue(name + " shows its cover in a poster view", cover instanceof PosterImageView);
        assertEquals(name + " crops its cover", ImageView.ScaleType.CENTER_CROP, ((ImageView) cover).getScaleType());
    }

    @Test
    public void everyPlaceACoverIsShownCropsIt() {
        try (var controller = Robolectric.buildActivity(MotionTest.Screen.class).setup().visible()) {
            Activity activity = controller.get();
            assertCoverFrame(activity, R.layout.console_card_cover, R.id.console_cover);
            assertCoverFrame(activity, R.layout.app_grid_item, R.id.grid_image);
            assertCoverFrame(activity, R.layout.app_grid_item_small, R.id.grid_image);
            assertCoverFrame(activity, R.layout.activity_game_detail, R.id.detail_cover);
            assertCoverFrame(activity, R.layout.activity_game_detail_tv, R.id.detail_cover);
        }
    }

    @Test
    @Config(qualifiers = "land")
    public void theSidewaysGamePageCropsItsCover() {
        try (var controller = Robolectric.buildActivity(MotionTest.Screen.class).setup().visible()) {
            assertCoverFrame(controller.get(), R.layout.activity_game_detail, R.id.detail_cover);
        }
    }

    @Test
    @Config(qualifiers = "television")
    public void televisionCoversCropToo() {
        try (var controller = Robolectric.buildActivity(MotionTest.Screen.class).setup().visible()) {
            Activity activity = controller.get();
            assertCoverFrame(activity, R.layout.console_card_cover, R.id.console_cover);
            assertCoverFrame(activity, R.layout.app_grid_item, R.id.grid_image);
            assertCoverFrame(activity, R.layout.app_grid_item_small, R.id.grid_image);
        }
    }
}

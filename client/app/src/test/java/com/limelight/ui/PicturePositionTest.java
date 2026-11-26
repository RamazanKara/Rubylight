package com.limelight.ui;

import android.app.Application;
import android.view.View;
import android.widget.FrameLayout;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import static org.junit.Assert.*;

/** Picture position: per-screen storage and the video's moved frame, which touch maps through. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 30, application = Application.class)
public class PicturePositionTest {
    @Test
    public void eachScreenKeepsItsOwnOffset() {
        // A foldable: inner and cover screens differ.
        String stored = PicturePosition.put(null, "1812x2176", 20, -5);
        stored = PicturePosition.put(stored, "1080x2520", 0, 40);
        assertArrayEquals(new int[] {20, -5}, PicturePosition.parse(stored, "1812x2176"));
        assertArrayEquals(new int[] {0, 40}, PicturePosition.parse(stored, "1080x2520"));
        assertArrayEquals(new int[] {0, 0}, PicturePosition.parse(stored, "1440x3120"));
        // Changing one replaces it; centring removes it.
        stored = PicturePosition.put(stored, "1812x2176", 30, 0);
        assertArrayEquals(new int[] {30, 0}, PicturePosition.parse(stored, "1812x2176"));
        assertEquals("1080x2520:0,40", PicturePosition.put(stored, "1812x2176", 0, 0));
        assertEquals("", PicturePosition.put("1080x2520:0,40", "1080x2520", 0, 0));
    }

    @Test
    public void damagedOrOutOfRangeValuesAreSafe() {
        assertArrayEquals(new int[] {0, 0}, PicturePosition.parse(null, "1080x2400"));
        assertArrayEquals(new int[] {0, 0}, PicturePosition.parse("1080x2400:x,y", "1080x2400"));
        assertArrayEquals(new int[] {0, 0}, PicturePosition.parse("1080x2400:5", "1080x2400"));
        assertArrayEquals(new int[] {150, -150}, PicturePosition.parse("garbage;1080x2400:900,-900", "1080x2400"));
        assertEquals("1080x2400:150,0", PicturePosition.put(null, "1080x2400", 999, 0));
    }

    @Test
    public void theVideoMovesOnScreenAndTouchFollowsItsFrame() {
        FrameLayout parent = new FrameLayout(RuntimeEnvironment.getApplication());
        StreamView video = new StreamView(RuntimeEnvironment.getApplication());
        video.setVideoScaling(VideoScaling.FIT, VideoScaling.CENTER, 1920, 1080);
        parent.addView(video, new FrameLayout.LayoutParams(-1, -1, android.view.Gravity.CENTER));
        layOut(parent);
        // 1920x1080 in 2400x1080 sits at 240 with no offset.
        assertEquals(240, video.getLeft());
        video.setPictureOffset(50, 0);
        layOut(parent);
        assertEquals(360, video.getLeft());
        assertEquals(360f, video.getX(), 0f);
        assertEquals(1920, video.getWidth());
        // Never past the screen's edge, and nothing to move into vertically.
        video.setPictureOffset(-150, 100);
        layOut(parent);
        assertEquals(0, video.getLeft());
        assertEquals(0, video.getTop());
        video.setPictureOffset(0, 0);
        layOut(parent);
        assertEquals(240, video.getLeft());
    }

    private static void layOut(View parent) {
        parent.measure(View.MeasureSpec.makeMeasureSpec(2400, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(1080, View.MeasureSpec.EXACTLY));
        parent.layout(0, 0, 2400, 1080);
    }
}

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
        assertEquals(240, video.getLeft());
        assertEquals(360f, video.getX(), 0f);
        assertEquals(1920, video.getWidth());
        // Never past the screen's edge, and nothing to move into vertically.
        video.setPictureOffset(-150, 100);
        layOut(parent);
        assertEquals(0f, video.getX(), 0f);
        assertEquals(0f, video.getY(), 0f);
        // The soft keyboard's pan adds to the picture's own move.
        video.setKeyboardPan(-200);
        assertEquals(-200f, video.getY(), 0f);
        video.setKeyboardPan(0);
        video.setPictureOffset(0, 0);
        layOut(parent);
        assertEquals(240f, video.getX(), 0f);
    }

    private static void layOut(View parent) {
        parent.measure(View.MeasureSpec.makeMeasureSpec(2400, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(1080, View.MeasureSpec.EXACTLY));
        parent.layout(0, 0, 2400, 1080);
    }

    @Test
    public void settingsEditsTheOffsetInTheStreamsOrientation() {
        // A phone held upright in Settings (rotation 0) streams in landscape (rotation 90).
        int rotation = PicturePosition.streamRotation(0, true, StreamOrientation.LANDSCAPE, false);
        assertEquals(1, rotation);
        // "Right" set in Settings is right in the stream, not a move down.
        int naturalX = SafeAreaPolicy.naturalOffsetX(50, 0, rotation), naturalY = SafeAreaPolicy.naturalOffsetY(50, 0, rotation);
        assertEquals(50, SafeAreaPolicy.screenOffsetX(naturalX, naturalY, 1));
        assertEquals(0, SafeAreaPolicy.screenOffsetY(naturalX, naturalY, 1));
        // Turned over by Flip 180° or the other landscape it stays on the same physical side.
        assertEquals(-50, SafeAreaPolicy.screenOffsetX(naturalX, naturalY, 3));
        // Automatic on a phone streams in landscape; already in a landscape, that one.
        assertEquals(1, PicturePosition.streamRotation(0, true, StreamOrientation.AUTO, false));
        assertEquals(3, PicturePosition.streamRotation(3, true, StreamOrientation.AUTO, false));
        assertEquals(1, PicturePosition.streamRotation(0, true, StreamOrientation.LANDSCAPE_FLIP, false));
        assertEquals(3, PicturePosition.streamRotation(3, true, StreamOrientation.LANDSCAPE_FLIP, false));
        assertEquals(3, PicturePosition.streamRotation(0, true, StreamOrientation.REVERSE_LANDSCAPE, false));
        assertEquals(3, PicturePosition.streamRotation(1, true, StreamOrientation.REVERSE_LANDSCAPE, false));
        // Portrait, Rotate and a screen that turns freely keep what Settings shows.
        assertEquals(0, PicturePosition.streamRotation(0, true, StreamOrientation.PORTRAIT, false));
        assertEquals(0, PicturePosition.streamRotation(1, true, StreamOrientation.PORTRAIT, false));
        assertEquals(2, PicturePosition.streamRotation(2, true, StreamOrientation.ROTATE, false));
        assertEquals(0, PicturePosition.streamRotation(0, true, StreamOrientation.AUTO, true));
        // A tablet or handheld whose panel is landscape at rotation 0.
        assertEquals(0, PicturePosition.streamRotation(0, false, StreamOrientation.LANDSCAPE, false));
        assertEquals(2, PicturePosition.streamRotation(0, false, StreamOrientation.REVERSE_LANDSCAPE, false));
        assertEquals(0, PicturePosition.streamRotation(1, false, StreamOrientation.AUTO, false));
        assertEquals(3, PicturePosition.streamRotation(0, false, StreamOrientation.PORTRAIT, false));
    }
}

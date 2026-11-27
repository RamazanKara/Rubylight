package com.limelight.ui;

import android.app.Application;
import android.content.pm.ActivityInfo;
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
        int rotation = PicturePosition.streamRotation(0, true, StreamOrientation.requested(StreamOrientation.LANDSCAPE,
                false, false, false, false, false, 1920, 1080));
        assertEquals(1, rotation);
        // "Right" set in Settings is right in the stream, not a move down.
        int naturalX = SafeAreaPolicy.naturalOffsetX(50, 0, rotation), naturalY = SafeAreaPolicy.naturalOffsetY(50, 0, rotation);
        assertEquals(50, SafeAreaPolicy.screenOffsetX(naturalX, naturalY, 1));
        assertEquals(0, SafeAreaPolicy.screenOffsetY(naturalX, naturalY, 1));
        // Turned over by Flip 180 degrees or the other landscape it stays on the same physical side.
        assertEquals(-50, SafeAreaPolicy.screenOffsetX(naturalX, naturalY, 3));
        // Landscape requests keep a landscape already shown, else take the system's.
        assertEquals(1, PicturePosition.streamRotation(0, true, ActivityInfo.SCREEN_ORIENTATION_USER_LANDSCAPE));
        assertEquals(3, PicturePosition.streamRotation(3, true, ActivityInfo.SCREEN_ORIENTATION_USER_LANDSCAPE));
        assertEquals(1, PicturePosition.streamRotation(0, true, ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE));
        assertEquals(3, PicturePosition.streamRotation(3, true, ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE));
        assertEquals(3, PicturePosition.streamRotation(0, true, ActivityInfo.SCREEN_ORIENTATION_REVERSE_LANDSCAPE));
        assertEquals(3, PicturePosition.streamRotation(1, true, ActivityInfo.SCREEN_ORIENTATION_REVERSE_LANDSCAPE));
        // Portrait requests and a window that turns freely.
        assertEquals(0, PicturePosition.streamRotation(0, true, ActivityInfo.SCREEN_ORIENTATION_USER_PORTRAIT));
        assertEquals(0, PicturePosition.streamRotation(1, true, ActivityInfo.SCREEN_ORIENTATION_USER_PORTRAIT));
        assertEquals(2, PicturePosition.streamRotation(2, true, ActivityInfo.SCREEN_ORIENTATION_FULL_USER));
        // A tablet or handheld whose panel is landscape at rotation 0.
        assertEquals(0, PicturePosition.streamRotation(0, false, ActivityInfo.SCREEN_ORIENTATION_USER_LANDSCAPE));
        assertEquals(2, PicturePosition.streamRotation(0, false, ActivityInfo.SCREEN_ORIENTATION_REVERSE_LANDSCAPE));
        assertEquals(3, PicturePosition.streamRotation(0, false, ActivityInfo.SCREEN_ORIENTATION_USER_PORTRAIT));
    }

    @Test
    public void aSquarishPhoneFollowsTheStreamsOwnRule() {
        // Automatic on a squarish phone held upright: on-screen controls ask for landscape, so Left / right
        // set in Settings must be saved for rotation 90, not as a vertical move.
        int withControls = StreamOrientation.requested(StreamOrientation.AUTO, false, false, true, true, false, 1920, 1080);
        assertEquals(1, PicturePosition.streamRotation(0, true, withControls));
        // A native stream keeps its shape: a landscape one turns the window, a portrait one does not.
        int nativeLandscape = StreamOrientation.requested(StreamOrientation.AUTO, false, false, true, false, true, 2208, 1840);
        assertEquals(1, PicturePosition.streamRotation(0, true, nativeLandscape));
        int nativePortrait = StreamOrientation.requested(StreamOrientation.AUTO, false, false, true, false, true, 1840, 2208);
        assertEquals(0, PicturePosition.streamRotation(0, true, nativePortrait));
        // Without either, the window turns freely and Settings' own rotation is the stream's.
        int free = StreamOrientation.requested(StreamOrientation.AUTO, false, false, true, false, false, 1920, 1080);
        assertEquals(0, PicturePosition.streamRotation(0, true, free));
        // A large screen turns freely too.
        assertEquals(0, PicturePosition.streamRotation(0, true,
                StreamOrientation.requested(StreamOrientation.AUTO, false, true, false, true, false, 1920, 1080)));
    }
}

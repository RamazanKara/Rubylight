package com.limelight.ui;

import android.content.pm.ActivityInfo;

import com.limelight.binding.video.StreamResizePolicy;

import org.junit.Test;

import static org.junit.Assert.*;

public class StreamOrientationTest {
    private static final int ANY = ActivityInfo.SCREEN_ORIENTATION_FULL_USER;
    private static final int LAND = ActivityInfo.SCREEN_ORIENTATION_USER_LANDSCAPE;
    private static final int PORT = ActivityInfo.SCREEN_ORIENTATION_USER_PORTRAIT;

    private static int phone(int mode) {
        return StreamOrientation.requested(mode, false, false, false, false, true, 2400, 1080);
    }

    @Test
    public void unknownOrMissingValuesAreAutomatic() {
        assertEquals(StreamOrientation.AUTO, StreamOrientation.fromValue(null));
        assertEquals(StreamOrientation.AUTO, StreamOrientation.fromValue("sideways"));
        for (int mode = StreamOrientation.AUTO; mode <= StreamOrientation.ROTATE; mode++) {
            assertEquals(mode, StreamOrientation.fromValue(StreamOrientation.value(mode)));
        }
    }

    @Test
    public void automaticKeepsTheLongStandingRule() {
        // Phones: landscape.
        assertEquals(LAND, phone(StreamOrientation.AUTO));
        // Tablets, desktop-class devices, windows and other displays: any orientation.
        assertEquals(ANY, StreamOrientation.requested(StreamOrientation.AUTO, false, true, false, true, true, 2400, 1080));
        assertEquals(ANY, StreamOrientation.requested(StreamOrientation.AUTO, true, false, false, true, true, 2400, 1080));
        // Squarish screens: a native stream keeps its shape, on-screen controls want landscape,
        // anything else turns freely.
        assertEquals(LAND, StreamOrientation.requested(StreamOrientation.AUTO, false, false, true, false, true, 2208, 1840));
        assertEquals(PORT, StreamOrientation.requested(StreamOrientation.AUTO, false, false, true, true, true, 1840, 2208));
        assertEquals(LAND, StreamOrientation.requested(StreamOrientation.AUTO, false, false, true, true, false, 1920, 1080));
        assertEquals(ANY, StreamOrientation.requested(StreamOrientation.AUTO, false, false, true, false, false, 1920, 1080));
    }

    @Test
    public void chosenOrientationsApplyToPhonesAndTabletsButNotToFreeWindows() {
        assertEquals(LAND, phone(StreamOrientation.LANDSCAPE));
        assertEquals(PORT, phone(StreamOrientation.PORTRAIT));
        assertEquals(ANY, phone(StreamOrientation.ROTATE));
        assertEquals(PORT, StreamOrientation.requested(StreamOrientation.PORTRAIT, false, true, false, false, true, 2560, 1600));
        assertEquals(LAND, StreamOrientation.requested(StreamOrientation.LANDSCAPE, false, false, true, false, true, 1840, 2208));
        // Split screen, desktop windows and video on another display cannot be turned by the app.
        for (int mode = StreamOrientation.AUTO; mode <= StreamOrientation.ROTATE; mode++) {
            assertEquals(ANY, StreamOrientation.requested(mode, true, false, false, false, true, 2400, 1080));
        }
    }

    @Test
    public void onlyAHostThatShapesTheStreamStartsItTurned() {
        // A host that scales its own screen would put a landscape desktop in a portrait frame.
        assertArrayEquals(new int[] {1920, 1080}, StreamOrientation.launchSize(StreamOrientation.PORTRAIT, false, true, 1920, 1080));
        assertArrayEquals(new int[] {1080, 1920}, StreamOrientation.launchSize(StreamOrientation.PORTRAIT, true, false, 1920, 1080));
        // Already portrait, or square: unchanged.
        assertArrayEquals(new int[] {1080, 1920}, StreamOrientation.launchSize(StreamOrientation.PORTRAIT, true, false, 1080, 1920));
        assertArrayEquals(new int[] {1440, 1440}, StreamOrientation.launchSize(StreamOrientation.PORTRAIT, true, false, 1440, 1440));
        // Automatic and Landscape never change the request.
        assertArrayEquals(new int[] {2400, 1080}, StreamOrientation.launchSize(StreamOrientation.AUTO, true, true, 2400, 1080));
        assertArrayEquals(new int[] {1080, 2400}, StreamOrientation.launchSize(StreamOrientation.LANDSCAPE, true, true, 1080, 2400));
    }

    @Test
    public void rotateStartsInTheWindowsShape() {
        assertArrayEquals(new int[] {1080, 2400}, StreamOrientation.launchSize(StreamOrientation.ROTATE, true, true, 2400, 1080));
        assertArrayEquals(new int[] {2400, 1080}, StreamOrientation.launchSize(StreamOrientation.ROTATE, true, false, 2400, 1080));
        assertArrayEquals(new int[] {2400, 1080}, StreamOrientation.launchSize(StreamOrientation.ROTATE, true, false, 1080, 2400));
        assertArrayEquals(new int[] {1080, 2400}, StreamOrientation.launchSize(StreamOrientation.ROTATE, true, true, 1080, 2400));
        assertArrayEquals(new int[] {2400, 1080}, StreamOrientation.launchSize(StreamOrientation.ROTATE, false, true, 2400, 1080));
    }

    @Test
    public void onlyRotateOnAShapingHostFollowsTheWindow() {
        assertTrue(StreamOrientation.followsRotation(StreamOrientation.ROTATE, true));
        assertFalse(StreamOrientation.followsRotation(StreamOrientation.ROTATE, false));
        assertFalse(StreamOrientation.followsRotation(StreamOrientation.AUTO, true));
        assertFalse(StreamOrientation.followsRotation(StreamOrientation.PORTRAIT, true));

        assertEquals(StreamResizePolicy.ORIENTATION_PORTRAIT,
                StreamOrientation.streamOrientation(StreamOrientation.ROTATE, true, true, false));
        assertEquals(StreamResizePolicy.ORIENTATION_LANDSCAPE,
                StreamOrientation.streamOrientation(StreamOrientation.ROTATE, true, false, true));
        // Everything else keeps the orientation the stream started in, as before.
        assertEquals(StreamResizePolicy.ORIENTATION_LANDSCAPE,
                StreamOrientation.streamOrientation(StreamOrientation.ROTATE, false, true, false));
        assertEquals(StreamResizePolicy.ORIENTATION_LANDSCAPE,
                StreamOrientation.streamOrientation(StreamOrientation.AUTO, true, true, false));
        assertEquals(StreamResizePolicy.ORIENTATION_PORTRAIT,
                StreamOrientation.streamOrientation(StreamOrientation.AUTO, true, false, true));
    }

    @Test
    public void turnedSizesSwapAndKeepWithinTheDecoder() {
        assertArrayEquals(new int[] {1080, 1920}, StreamOrientation.turnedSize(1920, 1080, false, 4096, 4096, 2, 2));
        assertArrayEquals(new int[] {1920, 1080}, StreamOrientation.turnedSize(1080, 1920, true, 4096, 4096, 2, 2));
        assertArrayEquals(new int[] {1920, 1080}, StreamOrientation.turnedSize(1920, 1080, true, 4096, 4096, 2, 2));
        // Odd panel sizes round down to even and to the decoder's alignment.
        assertArrayEquals(new int[] {1078, 2400}, StreamOrientation.turnedSize(2401, 1079, false, 4096, 4096, 2, 2));
        assertArrayEquals(new int[] {768, 1360}, StreamOrientation.turnedSize(1366, 768, false, 4096, 4096, 16, 16));
        // A decoder that is not as tall as the turned stream shrinks it, keeping the shape.
        assertArrayEquals(new int[] {606, 1080}, StreamOrientation.turnedSize(1920, 1080, false, 1920, 1080, 2, 2));
        // Nothing the host accepts fits.
        assertNull(StreamOrientation.turnedSize(1920, 1080, false, 1920, 200, 2, 2));
    }

    @Test
    public void aTurnWaitsLongerOnlyWhenItMayAskTheHost() {
        assertEquals(StreamOrientation.ROTATION_SETTLE_MS,
                StreamOrientation.settleMs(StreamOrientation.ROTATE, true, true, 300));
        assertEquals(300, StreamOrientation.settleMs(StreamOrientation.ROTATE, true, false, 300));
        assertEquals(300, StreamOrientation.settleMs(StreamOrientation.ROTATE, false, true, 300));
        assertEquals(300, StreamOrientation.settleMs(StreamOrientation.AUTO, true, true, 300));
    }

    @Test
    public void touchMapsThroughTheTurnedLayout() {
        // A landscape stream the host could not turn, fitted into a portrait phone: a touch on
        // the picture's centre and corners lands on the same stream pixels as in landscape.
        VideoScaling fit = new VideoScaling();
        fit.layout(VideoScaling.FIT, VideoScaling.CENTER, 1920, 1080, 1080, 2400);
        assertEquals(0, fit.left());
        assertEquals(1080, fit.width());
        assertEquals(608, fit.height());
        assertEquals(896, fit.top());
        assertEquals(960, VideoScaling.toStream(540, 1920, fit.left(), fit.width(), 1, 0), 0.5f);
        assertEquals(540, VideoScaling.toStream(896 + 304, 1080, fit.top(), fit.height(), 1, 0), 0.5f);
        // Touches on the bars map to the nearest edge.
        assertEquals(0, VideoScaling.toStream(100, 1080, fit.top(), fit.height(), 1, 0), 0.01f);
        assertEquals(1080, VideoScaling.toStream(2300, 1080, fit.top(), fit.height(), 1, 0), 0.01f);
        // After the host turned the stream, the same phone maps one to one.
        fit.layout(VideoScaling.FIT, VideoScaling.CENTER, 1080, 2400, 1080, 2400);
        assertEquals(1080, fit.width());
        assertEquals(2400, fit.height());
        assertEquals(1200, VideoScaling.toStream(1200, 2400, fit.top(), fit.height(), 1, 0), 0.01f);
    }
}

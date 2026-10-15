package com.limelight.ui;

import org.junit.Test;

import static org.junit.Assert.*;

public class VideoScalingTest {
    private static VideoScaling layout(int mode, int alignment, int videoWidth, int videoHeight, int areaWidth, int areaHeight) {
        VideoScaling scaling = new VideoScaling();
        scaling.layout(mode, alignment, videoWidth, videoHeight, areaWidth, areaHeight);
        return scaling;
    }

    private static void assertRect(VideoScaling scaling, int left, int top, int width, int height) {
        assertEquals("left", left, scaling.left());
        assertEquals("top", top, scaling.top());
        assertEquals("width", width, scaling.width());
        assertEquals("height", height, scaling.height());
    }

    @Test
    public void olderInstallsKeepTheirStretchChoice() {
        assertEquals(VideoScaling.FIT, VideoScaling.resolve(null, false));
        assertEquals(VideoScaling.STRETCH, VideoScaling.resolve(null, true));
        assertEquals(VideoScaling.FIT, VideoScaling.resolve("unknown", false));
        assertEquals(VideoScaling.STRETCH, VideoScaling.resolve("unknown", true));
    }

    @Test
    public void everyModeSurvivesARoundTrip() {
        for (int mode = VideoScaling.FIT; mode <= VideoScaling.NATIVE; mode++) {
            assertEquals(mode, VideoScaling.resolve(VideoScaling.value(mode), VideoScaling.legacyStretch(mode)));
        }
    }

    @Test
    public void newerModesLookLikeFitToOlderVersions() {
        assertFalse(VideoScaling.legacyStretch(VideoScaling.FIT));
        assertFalse(VideoScaling.legacyStretch(VideoScaling.FILL));
        assertFalse(VideoScaling.legacyStretch(VideoScaling.NATIVE));
        assertTrue(VideoScaling.legacyStretch(VideoScaling.STRETCH));
    }

    @Test
    public void aChoiceMadeInAnOlderVersionWins() {
        // Fill or 1:1 was saved, then an older version turned Stretch on.
        assertEquals(VideoScaling.STRETCH, VideoScaling.resolve("fill", true));
        assertEquals(VideoScaling.STRETCH, VideoScaling.resolve("native", true));
        // Stretch was saved, then an older version turned it off.
        assertEquals(VideoScaling.FIT, VideoScaling.resolve("stretch", false));
    }

    @Test
    public void alignmentDefaultsToCentre() {
        assertEquals(VideoScaling.CENTER, VideoScaling.alignment(null));
        assertEquals(VideoScaling.CENTER, VideoScaling.alignment("center"));
        assertEquals(VideoScaling.CENTER, VideoScaling.alignment("bottom"));
        assertEquals(VideoScaling.TOP, VideoScaling.alignment("top"));
    }

    @Test
    public void onlyFitAndNativeMoveToTheTop() {
        assertTrue(VideoScaling.alignsTop(VideoScaling.FIT, VideoScaling.TOP));
        assertTrue(VideoScaling.alignsTop(VideoScaling.NATIVE, VideoScaling.TOP));
        assertFalse(VideoScaling.alignsTop(VideoScaling.FILL, VideoScaling.TOP));
        assertFalse(VideoScaling.alignsTop(VideoScaling.STRETCH, VideoScaling.TOP));
        assertFalse(VideoScaling.alignsTop(VideoScaling.FIT, VideoScaling.CENTER));
    }

    @Test
    public void pictureInPictureShowsTheWholeStream() {
        assertEquals(VideoScaling.FIT, VideoScaling.forPictureInPicture(VideoScaling.FIT));
        assertEquals(VideoScaling.FIT, VideoScaling.forPictureInPicture(VideoScaling.FILL));
        assertEquals(VideoScaling.FIT, VideoScaling.forPictureInPicture(VideoScaling.NATIVE));
        assertEquals(VideoScaling.STRETCH, VideoScaling.forPictureInPicture(VideoScaling.STRETCH));
    }

    @Test
    public void onlyFillAndStretchCoverTheScreen() {
        assertTrue(VideoScaling.coversArea(VideoScaling.FILL));
        assertTrue(VideoScaling.coversArea(VideoScaling.STRETCH));
        assertFalse(VideoScaling.coversArea(VideoScaling.FIT));
        assertFalse(VideoScaling.coversArea(VideoScaling.NATIVE));
    }

    @Test
    public void fitLetterboxesAndPillarboxes() {
        // 16:9 on a 20:9 phone: bars left and right.
        assertRect(layout(VideoScaling.FIT, VideoScaling.CENTER, 1920, 1080, 2400, 1080), 240, 0, 1920, 1080);
        // 16:9 on a portrait-ish screen: bars above and below.
        assertRect(layout(VideoScaling.FIT, VideoScaling.CENTER, 1920, 1080, 1080, 2400), 0, 896, 1080, 608);
        // 16:10 on a near-square foldable inner screen.
        assertRect(layout(VideoScaling.FIT, VideoScaling.CENTER, 2560, 1600, 2208, 1768), 0, 194, 2208, 1380);
    }

    @Test
    public void fitOfThePanelsOwnSizeFillsItExactly() {
        // 2184 / 1968 is not exact in floating point; the result must not leave a 1 px line.
        assertRect(layout(VideoScaling.FIT, VideoScaling.CENTER, 2184, 1968, 2184, 1968), 0, 0, 2184, 1968);
        assertRect(layout(VideoScaling.FIT, VideoScaling.CENTER, 1968, 2184, 1968, 2184), 0, 0, 1968, 2184);
    }

    @Test
    public void fillCoversAndCropsEvenly() {
        // 16:9 on 20:9: full width, top and bottom cropped.
        assertRect(layout(VideoScaling.FILL, VideoScaling.CENTER, 1920, 1080, 2400, 1080), 0, -135, 2400, 1350);
        // 16:9 on a portrait screen: full height, sides cropped.
        assertRect(layout(VideoScaling.FILL, VideoScaling.CENTER, 1920, 1080, 1080, 2400), -1593, 0, 4267, 2400);
        // An odd aspect ratio (21:9 stream, 4:3 tablet).
        assertRect(layout(VideoScaling.FILL, VideoScaling.CENTER, 2560, 1080, 2048, 1536), -796, 0, 3641, 1536);
        // Top alignment does not change Fill.
        assertRect(layout(VideoScaling.FILL, VideoScaling.TOP, 1920, 1080, 2400, 1080), 0, -135, 2400, 1350);
    }

    @Test
    public void fillOfTheSameShapeIsFit() {
        for (int mode : new int[] {VideoScaling.FIT, VideoScaling.FILL, VideoScaling.STRETCH}) {
            assertRect(layout(mode, VideoScaling.CENTER, 1280, 720, 1920, 1080), 0, 0, 1920, 1080);
        }
    }

    @Test
    public void stretchFillsTheArea() {
        assertRect(layout(VideoScaling.STRETCH, VideoScaling.TOP, 1920, 1080, 2400, 1080), 0, 0, 2400, 1080);
        assertRect(layout(VideoScaling.STRETCH, VideoScaling.CENTER, 1920, 1080, 1080, 2400), 0, 0, 1080, 2400);
    }

    @Test
    public void nativeIsCentredAndCropsWhenLarger() {
        // Smaller than the screen: border all round.
        assertRect(layout(VideoScaling.NATIVE, VideoScaling.CENTER, 1280, 720, 2400, 1080), 560, 180, 1280, 720);
        // Larger than the screen: centred crop.
        assertRect(layout(VideoScaling.NATIVE, VideoScaling.CENTER, 3840, 2160, 2400, 1080), -720, -540, 3840, 2160);
        // Odd sizes round towards zero like FrameLayout gravity.
        assertRect(layout(VideoScaling.NATIVE, VideoScaling.CENTER, 1281, 721, 2400, 1080), 559, 179, 1281, 721);
        assertRect(layout(VideoScaling.NATIVE, VideoScaling.CENTER, 2401, 1081, 2400, 1080), 0, 0, 2401, 1081);
    }

    @Test
    public void topAlignmentKeepsTheTopEdgeAndCentresAcross() {
        assertRect(layout(VideoScaling.FIT, VideoScaling.TOP, 1920, 1080, 1080, 2400), 0, 0, 1080, 608);
        assertRect(layout(VideoScaling.FIT, VideoScaling.TOP, 1920, 1080, 2400, 1080), 240, 0, 1920, 1080);
        assertRect(layout(VideoScaling.NATIVE, VideoScaling.TOP, 1280, 720, 2400, 1080), 560, 0, 1280, 720);
        // Larger than the screen: only the bottom is cropped.
        assertRect(layout(VideoScaling.NATIVE, VideoScaling.TOP, 3840, 2160, 2400, 1080), -720, 0, 3840, 2160);
    }

    @Test
    public void anUnknownVideoOrAreaFillsWhatThereIs() {
        for (int mode = VideoScaling.FIT; mode <= VideoScaling.NATIVE; mode++) {
            assertRect(layout(mode, VideoScaling.CENTER, 0, 0, 2400, 1080), 0, 0, 2400, 1080);
            assertRect(layout(mode, VideoScaling.CENTER, 1920, 1080, 0, 0), 0, 0, 0, 0);
        }
    }

    @Test
    public void screenToStreamWithoutZoom() {
        // Fit 1920x1080 on 2400x1080: the picture starts at x 240.
        assertEquals(0, VideoScaling.toStream(240, 1920, 240, 1920, 1, 0), 0.001f);
        assertEquals(960, VideoScaling.toStream(1200, 1920, 240, 1920, 1, 0), 0.001f);
        assertEquals(1920, VideoScaling.toStream(2160, 1920, 240, 1920, 1, 0), 0.001f);
        // Fill 1920x1080 on 2400x1080: the rectangle starts above the screen.
        assertEquals(540, VideoScaling.toStream(540, 1080, -135, 1350, 1, 0), 0.001f);
        assertEquals(108, VideoScaling.toStream(0, 1080, -135, 1350, 1, 0), 0.001f);
        assertEquals(972, VideoScaling.toStream(1080, 1080, -135, 1350, 1, 0), 0.001f);
        // 1:1 is one stream pixel per screen pixel.
        assertEquals(1000, VideoScaling.toStream(1560, 1280, 560, 1280, 1, 0), 0.001f);
    }

    @Test
    public void pointsOnABorderClampToTheNearestEdge() {
        // The left border of a Fit picture and the border around a small 1:1 picture.
        assertEquals(0, VideoScaling.toStream(100, 1920, 240, 1920, 1, 0), 0);
        assertEquals(1920, VideoScaling.toStream(2399, 1920, 240, 1920, 1, 0), 0);
        assertEquals(0, VideoScaling.toStream(0, 720, 180, 720, 1, 0), 0);
        assertEquals(720, VideoScaling.toStream(1079, 720, 180, 720, 1, 0), 0);
    }

    @Test
    public void zoomComposesWithTheScalingRectangle() {
        // Fit at x 240, zoomed 2x about the rectangle's own centre (pan -960 in rectangle coordinates).
        float scale = 2, pan = -960;
        // The screen centre still shows the stream centre.
        assertEquals(960, VideoScaling.toStream(1200, 1920, 240, 1920, scale, pan), 0.001f);
        // The rectangle's left edge now shows a quarter of the way in.
        assertEquals(480, VideoScaling.toStream(240, 1920, 240, 1920, scale, pan), 0.001f);
        // Fill, zoomed 4x with the rectangle's top-left at the screen origin of the zoomed content.
        assertEquals(27, VideoScaling.toStream(0, 1080, -135, 1350, 4, 0), 0.001f);
    }

    @Test
    public void screenAndStreamRoundTrip() {
        int[][] rects = {{240, 1920, 1920}, {-135, 1350, 1080}, {560, 1280, 1280}, {-720, 3840, 3840}, {0, 1080, 720}};
        float[][] zooms = {{1, 0}, {2, -500}, {4, -1000}, {1.5f, -12.5f}};
        for (int[] rect : rects) {
            for (float[] zoom : zooms) {
                // Pan as DesktopViewport would allow it: the zoomed picture still covers its rectangle.
                float pan = Math.max(rect[1] * (1 - zoom[0]), Math.min(0, zoom[1]));
                for (float stream = 0; stream <= rect[2]; stream += rect[2] / 7f) {
                    float screen = VideoScaling.toScreen(stream, rect[2], rect[0], rect[1], zoom[0], pan);
                    assertEquals(stream, VideoScaling.toStream(screen, rect[2], rect[0], rect[1], zoom[0], pan), 0.01f);
                }
            }
        }
    }

    @Test
    public void degenerateInputMapsToTheOrigin() {
        assertEquals(0, VideoScaling.toStream(100, 0, 0, 100, 1, 0), 0);
        assertEquals(0, VideoScaling.toStream(100, 100, 0, 0, 1, 0), 0);
        assertEquals(0, VideoScaling.toStream(100, 100, 0, 100, 0, 0), 0);
        assertEquals(0, VideoScaling.toStream(100, 100, 0, 100, Float.NaN, 0), 0);
        assertEquals(240, VideoScaling.toScreen(10, 0, 240, 1920, 1, 0), 0);
    }
}

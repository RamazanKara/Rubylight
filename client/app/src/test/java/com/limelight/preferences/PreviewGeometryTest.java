package com.limelight.preferences;

import com.limelight.ui.VideoScaling;

import org.junit.Test;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

/** The preview pictures' geometry: each drawing says what its choice does, and never something the choice does not. */
public class PreviewGeometryTest {
    private static final float E = 1e-4f;

    private static float gap(int choice, int frame) {
        return PreviewPacing.shown(choice, frame + 1) - PreviewPacing.shown(choice, frame);
    }

    @Test
    public void thePacingPictureKnowsEveryStoredValue() {
        assertEquals(FramePacingChoice.VALUES.length, PreviewPacing.VALUES.length);
        for (String value : FramePacingChoice.VALUES) {
            assertEquals(value, PreviewPacing.VALUES[PreviewPacing.choice(value)]);
        }
        assertEquals("an unknown value draws as the default", PreviewPacing.LATENCY, PreviewPacing.choice("later"));
        assertEquals(PreviewPacing.LATENCY, PreviewPacing.choice(null));
    }

    @Test
    public void noFrameIsShownBeforeItArrivesAndLowestLatencyShowsItAtOnce() {
        for (int choice = 0; choice < PreviewPacing.VALUES.length; choice++) {
            for (int frame = 0; frame < PreviewPacing.FRAMES; frame++) {
                float shown = PreviewPacing.shown(choice, frame);
                assertTrue(PreviewPacing.VALUES[choice], shown >= PreviewPacing.arrival(frame) - E);
                assertTrue("on the timeline", shown >= 0f && shown <= PreviewPacing.TICKS);
            }
        }
        for (int frame = 0; frame < PreviewPacing.FRAMES; frame++) {
            assertEquals(PreviewPacing.arrival(frame), PreviewPacing.shown(PreviewPacing.LATENCY, frame), E);
        }
    }

    @Test
    public void lowestLatencyIsUnevenAndEveryOtherChoiceIsEven() {
        float least = Float.MAX_VALUE, most = 0f;
        for (int frame = 0; frame < PreviewPacing.FRAMES - 1; frame++) {
            least = Math.min(least, gap(PreviewPacing.LATENCY, frame));
            most = Math.max(most, gap(PreviewPacing.LATENCY, frame));
        }
        assertTrue("the network's jitter shows", most - least > 0.3f);
        for (int choice = PreviewPacing.BALANCED; choice < PreviewPacing.VALUES.length; choice++) {
            for (int frame = 1; frame < PreviewPacing.FRAMES - 1; frame++) {
                assertEquals(PreviewPacing.VALUES[choice], gap(choice, 0), gap(choice, frame), E);
            }
        }
    }

    @Test
    public void theBalancedChoicesShowOneFramePerRefreshEachWaitingLonger() {
        int[] waiting = {PreviewPacing.BALANCED, PreviewPacing.BALANCED_EVERY, PreviewPacing.SMOOTHNESS};
        for (int choice : waiting) {
            for (int frame = 0; frame < PreviewPacing.FRAMES; frame++) {
                float shown = PreviewPacing.shown(choice, frame);
                assertEquals("on a refresh tick", Math.round(shown), shown, E);
            }
            assertEquals(1f, gap(choice, 0), E);
        }
        assertTrue(PreviewPacing.shown(PreviewPacing.BALANCED, 0) < PreviewPacing.shown(PreviewPacing.BALANCED_EVERY, 0));
        assertTrue(PreviewPacing.shown(PreviewPacing.BALANCED_EVERY, 0) < PreviewPacing.shown(PreviewPacing.SMOOTHNESS, 0));
    }

    @Test
    public void theFpsLimitSpacesFramesWiderThanTheRefreshAndThePcsTimesKeepItsPace() {
        assertEquals(PreviewPacing.CAP_INTERVAL, gap(PreviewPacing.CAP_FPS, 0), E);
        assertTrue(PreviewPacing.CAP_INTERVAL > 1f);
        assertEquals(1f, gap(PreviewPacing.HOST, 0), E);
        float first = PreviewPacing.shown(PreviewPacing.HOST, 0);
        assertTrue("a moment later, before the next refresh", first > 0f && first < 1f);
    }

    @Test
    public void withoutHdrThePictureClipsAtAStandardScreensWhite() {
        for (float scene = 0f; scene <= 1f; scene += 0.05f) {
            float level = PreviewTone.level(PreviewTone.SDR, scene);
            assertEquals(Math.min(scene, PreviewTone.SDR_WHITE), level, E);
        }
        assertEquals(PreviewTone.level(PreviewTone.SDR, 0.7f), PreviewTone.level(PreviewTone.SDR, 1f), E);
    }

    @Test
    public void toneMappingBendsIntoThatWhiteWithoutClipping() {
        float before = -1f;
        for (int step = 0; step <= 40; step++) {
            float scene = step / 40f;
            float level = PreviewTone.level(PreviewTone.TONE_MAPPED, scene);
            assertTrue("always rising: no highlight is clipped", level > before);
            assertTrue(level < PreviewTone.SDR_WHITE);
            if (scene <= PreviewTone.KNEE) assertEquals(scene, level, E);
            else assertTrue("the shoulder", level < scene);
            before = level;
        }
        assertEquals("no step at the knee", PreviewTone.level(PreviewTone.TONE_MAPPED, PreviewTone.KNEE),
                PreviewTone.level(PreviewTone.TONE_MAPPED, PreviewTone.KNEE + 0.001f), 0.002f);
    }

    @Test
    public void hdrKeepsTheWholeRange() {
        assertEquals(1f, PreviewTone.level(PreviewTone.HDR, 1f), E);
        assertEquals(0f, PreviewTone.level(PreviewTone.HDR, 0f), E);
        assertTrue(PreviewTone.level(PreviewTone.HDR, 0.9f) > PreviewTone.level(PreviewTone.TONE_MAPPED, 0.9f));
        assertTrue(PreviewTone.level(PreviewTone.TONE_MAPPED, 0.9f) < PreviewTone.level(PreviewTone.SDR, 0.9f) + E);
    }

    @Test
    public void theSharpModesAreCrisperThanOffAndSmoothAndFollowTheSlider() {
        for (String sharp : new String[] {"fsr1", "sgsr1"}) {
            assertEquals(0f, PreviewEdge.softness(sharp, 100), E);
            assertEquals(PreviewEdge.SHARP_SOFTEST, PreviewEdge.softness(sharp, 0), E);
            assertTrue(PreviewEdge.softness(sharp, 80) < PreviewEdge.softness(sharp, 20));
            assertTrue(PreviewEdge.softness(sharp, 0) < PreviewEdge.softness("off", 0));
            assertEquals("out of range clamps", 0f, PreviewEdge.softness(sharp, 400), E);
        }
        assertEquals("the slider is not used without a sharp mode", PreviewEdge.softness("bilinear", 0),
                PreviewEdge.softness("bilinear", 100), E);
        assertEquals(PreviewEdge.softness("off", 0), PreviewEdge.softness("off", 100), E);
        assertTrue("Smooth is the softest", PreviewEdge.softness("bilinear", 50) > PreviewEdge.softness("off", 50));
        assertEquals("an unknown mode draws as Off", PreviewEdge.softness("off", 50), PreviewEdge.softness(null, 50), E);
    }

    @Test
    public void aCrispEdgeIsAllOrNothingAndASoftOneSpreadsOverItsNeighbours() {
        assertEquals(1f, PreviewEdge.coverage(0.5f, 0f), E);
        assertEquals(0f, PreviewEdge.coverage(-0.5f, 0f), E);
        float soft = PreviewEdge.coverage(0.5f, 1f);
        assertTrue(soft > 0.5f && soft < 1f);
        assertEquals("the edge itself is half covered either way", PreviewEdge.coverage(0f, 0f), PreviewEdge.coverage(0f, 1f), E);
        float before = -1f;
        for (float distance = -2f; distance <= 2f; distance += 0.25f) {
            float share = PreviewEdge.coverage(distance, 0.6f);
            assertTrue(share >= before && share >= 0f && share <= 1f);
            before = share;
        }
    }

    @Test
    public void theEdgeRunsFromTheGridsBottomLeftToItsTopRight() {
        int columns = PreviewEdge.columns(240f, 60f);
        assertTrue(columns >= PreviewEdge.ROWS && columns <= 16);
        assertEquals("a box too narrow for square pixels still has a grid", PreviewEdge.ROWS, PreviewEdge.columns(20f, 60f));
        assertTrue("the bottom right is the bright side", PreviewEdge.distance(columns - 1, PreviewEdge.ROWS - 1, columns) > 1f);
        assertTrue(PreviewEdge.distance(0, 0, columns) < -1f);
        assertEquals("the two ends of the diagonal mirror each other", -PreviewEdge.distance(0, PreviewEdge.ROWS - 1, columns),
                PreviewEdge.distance(columns - 1, 0, columns), E);
    }

    @Test
    public void fitShowsAllOfThePictureAndFillCoversTheScreenBothUndistorted() {
        assertEquals(1f, PreviewScaling.width(VideoScaling.FIT), E);
        assertTrue("bars above and below", PreviewScaling.height(VideoScaling.FIT) < 1f);
        assertEquals(1f, PreviewScaling.roundness(VideoScaling.FIT), E);
        assertEquals(1f, PreviewScaling.height(VideoScaling.FILL), E);
        assertTrue("the sides are cut off", PreviewScaling.width(VideoScaling.FILL) > 1f);
        assertEquals(1f, PreviewScaling.roundness(VideoScaling.FILL), E);
    }

    @Test
    public void stretchCoversTheScreenByDistortingAndOneToOneLeavesABorder() {
        assertEquals(1f, PreviewScaling.width(VideoScaling.STRETCH), E);
        assertEquals(1f, PreviewScaling.height(VideoScaling.STRETCH), E);
        assertTrue("a circle is squeezed", PreviewScaling.roundness(VideoScaling.STRETCH) < 0.9f);
        assertTrue(PreviewScaling.width(VideoScaling.NATIVE) < 1f);
        assertTrue(PreviewScaling.height(VideoScaling.NATIVE) < PreviewScaling.height(VideoScaling.FIT));
        assertEquals(1f, PreviewScaling.roundness(VideoScaling.NATIVE), E);
    }

    @Test
    public void safeFitInsetsAllSidesAndKeepsAspectRatio() {
        assertTrue("smaller than Fit horizontally", PreviewScaling.width(VideoScaling.SAFE_FIT) < PreviewScaling.width(VideoScaling.FIT));
        assertTrue("smaller than Fit vertically", PreviewScaling.height(VideoScaling.SAFE_FIT) < PreviewScaling.height(VideoScaling.FIT));
        assertTrue("larger than 1:1 horizontally", PreviewScaling.width(VideoScaling.SAFE_FIT) > PreviewScaling.width(VideoScaling.NATIVE));
        assertTrue("larger than 1:1 vertically", PreviewScaling.height(VideoScaling.SAFE_FIT) > PreviewScaling.height(VideoScaling.NATIVE));
        assertEquals(1f, PreviewScaling.roundness(VideoScaling.SAFE_FIT), E);
    }
}

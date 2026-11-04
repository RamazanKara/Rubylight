package com.limelight.binding.video;

import org.junit.Test;

import java.io.BufferedReader;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;

import static org.junit.Assert.*;

public class StreamResizePolicyTest {
    private static final int LANDSCAPE = StreamResizePolicy.ORIENTATION_LANDSCAPE;
    private static final int PORTRAIT = StreamResizePolicy.ORIENTATION_PORTRAIT;
    private static final int AREA = StreamResizePolicy.ORIENTATION_OF_AREA;

    @Test
    public void foldPanelsKeepTheStreamsLandscapeOrientation() {
        // Galaxy Z Fold5: inner 2176x1812, cover 904x2316 reported in portrait.
        assertArrayEquals(new int[] {2176, 1812},
                StreamResizePolicy.nativeSize(1812, 2176, 1, LANDSCAPE, 8192, 8192, 2, 2));
        assertArrayEquals(new int[] {2316, 904},
                StreamResizePolicy.nativeSize(904, 2316, 1, LANDSCAPE, 8192, 8192, 2, 2));
        assertArrayEquals(new int[] {1812, 2176},
                StreamResizePolicy.nativeSize(2176, 1812, 1, PORTRAIT, 8192, 8192, 2, 2));
    }

    @Test
    public void windowsFollowTheirOwnShapeAndPanelPixels() {
        assertArrayEquals(new int[] {1086, 1812},
                StreamResizePolicy.nativeSize(1086, 1812, 1, AREA, 8192, 8192, 2, 2));
        // A window on a panel rendered below its pixel resolution asks for the panel's pixels.
        assertArrayEquals(new int[] {1440, 1620},
                StreamResizePolicy.nativeSize(1080, 1215, 4f / 3, AREA, 8192, 8192, 2, 2));
    }

    @Test
    public void sizesAreEvenAndMatchTheDecodersAlignment() {
        assertArrayEquals(new int[] {1086, 1810},
                StreamResizePolicy.nativeSize(1087, 1811, 1, AREA, 8192, 8192, 1, 1));
        assertArrayEquals(new int[] {1072, 1808},
                StreamResizePolicy.nativeSize(1087, 1811, 1, AREA, 8192, 8192, 16, 16));
        // An odd alignment still yields even sizes.
        assertArrayEquals(new int[] {1086, 1806},
                StreamResizePolicy.nativeSize(1087, 1811, 1, AREA, 8192, 8192, 3, 3));
    }

    @Test
    public void largeAreasShrinkToTheDecoderKeepingTheirShape() {
        assertArrayEquals(new int[] {4096, 2048},
                StreamResizePolicy.nativeSize(5120, 2560, 1, LANDSCAPE, 4096, 4096, 2, 2));
        assertArrayEquals(new int[] {3840, 1620},
                StreamResizePolicy.nativeSize(5120, 2160, 1, LANDSCAPE, 3840, 2160, 2, 2));
        int[] capped = StreamResizePolicy.nativeSize(16000, 9000, 1, LANDSCAPE, 100000, 100000, 2, 2);
        assertTrue(capped[0] <= StreamResizePolicy.MAX_DIMENSION && capped[1] <= StreamResizePolicy.MAX_DIMENSION);
    }

    @Test
    public void unusableAreasAskForNothing() {
        assertNull(StreamResizePolicy.nativeSize(0, 1080, 1, AREA, 8192, 8192, 2, 2));
        assertNull(StreamResizePolicy.nativeSize(1920, -1, 1, AREA, 8192, 8192, 2, 2));
        assertNull(StreamResizePolicy.nativeSize(1920, 1080, 0, AREA, 8192, 8192, 2, 2));
        assertNull(StreamResizePolicy.nativeSize(1920, 1080, Float.NaN, AREA, 8192, 8192, 2, 2));
        assertNull(StreamResizePolicy.nativeSize(1920, 1080, 1, AREA, 0, 8192, 2, 2));
        // A sliver of a window is below what the host accepts.
        assertNull(StreamResizePolicy.nativeSize(1920, 200, 1, AREA, 8192, 8192, 2, 2));
        assertNotNull(StreamResizePolicy.nativeSize(256, 256, 1, AREA, 8192, 8192, 2, 2));
    }

    @Test
    public void frameRateFollowsOnlyAMovedPanel() {
        assertEquals(120_000, StreamResizePolicy.fpsMillihz(120));
        assertEquals(59_940, StreamResizePolicy.fpsMillihz(59.94f));
        assertEquals(119_880, StreamResizePolicy.fpsMillihz(119.88f));
        assertEquals(23_976, StreamResizePolicy.fpsMillihz(23.976f));
        assertEquals(StreamResizePolicy.MIN_FPS_MILLIHZ, StreamResizePolicy.fpsMillihz(1));
        assertEquals(StreamResizePolicy.MAX_FPS_MILLIHZ, StreamResizePolicy.fpsMillihz(1000));
        // Same panel: keep what the stream started with, including a capped 119 fps.
        assertEquals(119_000, StreamResizePolicy.targetFpsMillihz(true, 119_000, 120, 120.001f));
        assertEquals(60_000, StreamResizePolicy.targetFpsMillihz(true, 120_000, 120, 60));
        assertEquals(120_000, StreamResizePolicy.targetFpsMillihz(false, 120_000, 120, 60));
        assertEquals(120_000, StreamResizePolicy.targetFpsMillihz(true, 120_000, 120, Float.NaN));
    }

    @Test
    public void fractionalPanelChangesPreserveTheirCadenceInBothDirections() {
        assertEquals(59_940, StreamResizePolicy.targetFpsMillihz(true, 60_000, 60, 59.94f));
        assertEquals(60_000, StreamResizePolicy.targetFpsMillihz(true, 59_940, 59.94f, 60));
        assertEquals(119_880, StreamResizePolicy.targetFpsMillihz(true, 120_000, 120, 119.88f));
        assertEquals(120_000, StreamResizePolicy.targetFpsMillihz(true, 119_880, 119.88f, 120));
        assertEquals(23_976, StreamResizePolicy.targetFpsMillihz(true, 24_000, 24, 23.976f));
        assertEquals(120_000, StreamResizePolicy.targetFpsMillihz(false, 120_000, 120, 119.88f));
    }

    @Test
    public void smallReportingNoiseAndInvalidRatesCannotChangeAnActiveStream() {
        for (float panel : new float[] {119.87999f, 119.88001f, 119.889f}) {
            assertEquals(119_000, StreamResizePolicy.targetFpsMillihz(true, 119_000, 119.88f, panel));
        }
        // A complete 0.01 Hz step is meaningful at the launch profile's precision.
        assertEquals(119_890, StreamResizePolicy.targetFpsMillihz(true, 119_880, 119.88f, 119.89f));
        for (float panel : new float[] {0, -1, Float.NaN, Float.POSITIVE_INFINITY, Float.NEGATIVE_INFINITY}) {
            assertEquals(59_940, StreamResizePolicy.targetFpsMillihz(true, 59_940, 59.94f, panel));
        }
        assertEquals(59_940, StreamResizePolicy.targetFpsMillihz(true, 60_000, Float.NaN, 59.94f));
        assertEquals(StreamResizePolicy.MAX_FPS_MILLIHZ,
                StreamResizePolicy.targetFpsMillihz(true, 120_000, 120, Float.MAX_VALUE));
        assertEquals(StreamResizePolicy.MIN_FPS_MILLIHZ,
                StreamResizePolicy.targetFpsMillihz(true, 60_000, 60, 1));
    }

    @Test
    public void visibleSizeUsesTheCropWhenReported() {
        assertArrayEquals(new int[] {2316, 904}, StreamResizePolicy.visibleSize(2320, 912, 0, 2315, 0, 903));
        assertArrayEquals(new int[] {1920, 1088}, StreamResizePolicy.visibleSize(1920, 1088, -1, -1, -1, -1));
        assertArrayEquals(new int[] {1920, 1080}, StreamResizePolicy.visibleSize(1920, 1088, -1, -1, 0, 1079));
    }

    @Test
    public void blockPaddingReadsAsTheExpectedSize() {
        assertArrayEquals(new int[] {1920, 1080}, StreamResizePolicy.shownSize(1920, 1088, 1920, 1080));
        assertArrayEquals(new int[] {2316, 904}, StreamResizePolicy.shownSize(2320, 912, 2316, 904));
        // A host that kept another size is shown as it is.
        assertArrayEquals(new int[] {2176, 1812}, StreamResizePolicy.shownSize(2176, 1812, 2316, 904));
        assertArrayEquals(new int[] {1904, 1080}, StreamResizePolicy.shownSize(1904, 1080, 1920, 1080));
        // AMD's AV1 encoder pads widths to 64: a Fold7's inner panel arrives 2240 wide.
        assertArrayEquals(new int[] {2184, 1968}, StreamResizePolicy.shownSize(2240, 1968, 2184, 1968));
        assertArrayEquals(new int[] {2248, 1968}, StreamResizePolicy.shownSize(2248, 1968, 2184, 1968));
        // At most 15 rows, as desktop Nightjar: 16 more is a stream at another size.
        assertArrayEquals(new int[] {1920, 1080}, StreamResizePolicy.shownSize(1920, 1095, 1920, 1080));
        assertArrayEquals(new int[] {1920, 1096}, StreamResizePolicy.shownSize(1920, 1096, 1920, 1080));
    }

    @Test
    public void theViewEnlargesOnlyAPictureTheUpscalerDoesNotCrop() {
        int[] decoded = {1920, 1082}, shown = {1920, 1080};
        assertArrayEquals(decoded, StreamResizePolicy.viewPaddedSize(decoded, shown, false));
        assertArrayEquals(shown, StreamResizePolicy.viewPaddedSize(decoded, shown, true));
    }

    @Test
    public void paddedVisibleSizeCropsAsNightjarCore() throws Exception {
        // The cases nightjar-core's tests/visible_size_contract.rs checks visible_size against.
        int cases = 0;
        try (InputStream input = getClass().getResourceAsStream("/protocol/visible-size.txt")) {
            assertNotNull("visible-size.txt", input);
            BufferedReader reader = new BufferedReader(new InputStreamReader(input, StandardCharsets.UTF_8));
            for (String line; (line = reader.readLine()) != null; ) {
                if (line.isEmpty() || line.startsWith("#")) continue;
                String[] fields = line.split(" ");
                assertEquals(line, 3, fields.length);
                String[] listed = fields[1].equals("-") ? new String[0] : fields[1].split(",");
                int[][] requested = new int[listed.length][];
                for (int i = 0; i < listed.length; i++) requested[i] = size(listed[i]);
                int[] decoded = size(fields[0]);
                assertArrayEquals(line, size(fields[2]),
                        StreamResizePolicy.paddedVisibleSize(decoded[0], decoded[1], requested));
                cases++;
            }
        }
        assertTrue("only " + cases + " cases", cases >= 20);
    }

    private static int[] size(String text) {
        String[] parts = text.split("x");
        return new int[] {Integer.parseInt(parts[0]), Integer.parseInt(parts[1])};
    }

    @Test
    public void adaptiveMaximumCoversBothFoldPanels() {
        assertArrayEquals(new int[] {2316, 1812}, StreamResizePolicy.coveringMax(2176, 1812, 2316, 904));
        assertArrayEquals(new int[] {2316, 1812}, StreamResizePolicy.coveringMax(2316, 1812, 2176, 1812));
    }
}

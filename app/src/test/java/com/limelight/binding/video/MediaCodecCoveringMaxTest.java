package com.limelight.binding.video;

import org.junit.Test;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Paths;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

/**
 * The adaptive maximum configured up front for a fold or turn: only a size the decoder takes, and
 * a refused configure retries at the stream's own maximum, so the covering never costs a decoder.
 */
public class MediaCodecCoveringMaxTest {
    private static final MediaCodecDecoderRenderer.SizeCheck ANY = (width, height) -> true;

    @Test
    public void theMaximumCoversTheStreamAndWhatItFoldsOrTurnsTo() {
        assertArrayEquals(new int[] {2316, 1812},
                MediaCodecDecoderRenderer.upFrontMax(2176, 1812, new int[] {2316, 1812}, 2, 2, ANY, false));
        assertArrayEquals(new int[] {1920, 1920},
                MediaCodecDecoderRenderer.upFrontMax(1920, 1080, new int[] {1920, 1920}, 2, 2, ANY, false));
    }

    @Test
    public void withoutACoveringSizeOrAlreadyCoveredTheStreamSizeStays() {
        assertArrayEquals(new int[] {1920, 1080},
                MediaCodecDecoderRenderer.upFrontMax(1920, 1080, null, 2, 2, ANY, false));
        assertArrayEquals(new int[] {1920, 1080},
                MediaCodecDecoderRenderer.upFrontMax(1920, 1080, new int[] {1280, 720}, 2, 2, ANY, false));
    }

    @Test
    public void theMaximumIsRoundedUpToTheDecodersAlignment() {
        int[][] asked = new int[1][];
        int[] max = MediaCodecDecoderRenderer.upFrontMax(2176, 1812, new int[] {2316, 1812}, 16, 16,
                (width, height) -> {
                    asked[0] = new int[] {width, height};
                    return true;
                }, false);
        assertArrayEquals(new int[] {2320, 1824}, max);
        assertArrayEquals("the decoder is asked about the size configured", max, asked[0]);
    }

    @Test
    public void aSizeTheDecoderRefusesKeepsTheStreamSize() {
        // A 4K-limited decoder and a turned 4K stream: 3840x3840 is beyond it.
        MediaCodecDecoderRenderer.SizeCheck fourK = (width, height) -> width <= 3840 && height <= 2160;
        assertArrayEquals(new int[] {3840, 2160},
                MediaCodecDecoderRenderer.upFrontMax(3840, 2160, new int[] {3840, 3840}, 2, 2, fourK, false));
        // H.264 stops at 4096 whatever the decoder lists.
        assertArrayEquals(new int[] {3840, 2160},
                MediaCodecDecoderRenderer.upFrontMax(3840, 2160, new int[] {4320, 4320}, 2, 2, ANY, true));
        assertArrayEquals(new int[] {4320, 4320},
                MediaCodecDecoderRenderer.upFrontMax(3840, 2160, new int[] {4320, 4320}, 2, 2, ANY, false));
    }

    @Test
    public void theSwitchDefaultsOn() {
        assertEquals("covering_max", MediaCodecDecoderRenderer.COVERING_SWITCH);
    }

    // initializeDecoder needs a real codec ladder, so this pins the wiring: only decoders with
    // adaptive playback get the covering, and a refusal retries the same try at the stream's
    // maximum instead of throwing or giving up on the decoder.
    @Test
    public void aRefusedCoveringMaximumRetriesAtTheStreamsOwn() throws Exception {
        String source = new String(Files.readAllBytes(
                Paths.get("src/main/java/com/limelight/binding/video/MediaCodecDecoderRenderer.java")), StandardCharsets.UTF_8);
        assertTrue(source.contains("MediaCodecHelper.debugSwitch(COVERING_SWITCH, true)"));
        assertTrue(source.contains("if (adaptivePlayback && coveringSize != null && !coveringRefused"));
        assertTrue("a try with the covering never throws",
                source.contains("throwOnCodecError && !dolbyVision && !coveringApplied)"));
        int retry = source.indexOf("if (coveringApplied) {");
        int giveUp = source.indexOf("if (!newFormat) {", retry);
        assertTrue("the retry comes before the ladder gives up", retry > 0 && giveUp > retry);
        String block = source.substring(retry, giveUp);
        assertTrue(block.contains("maxDecodeWidth = streamMax[0];"));
        assertTrue(block.contains("coveringRefused = true;"));
        assertTrue(block.contains("tryNumber--;"));
    }
}

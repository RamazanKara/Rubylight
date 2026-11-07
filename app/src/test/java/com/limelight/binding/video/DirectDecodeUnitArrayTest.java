package com.limelight.binding.video;

import org.junit.Test;

import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Paths;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotSame;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertTrue;

/**
 * A frame handed over in place reaches submitDecodeUnit without an array. When Dolby Vision, the
 * HDR Vivid SEI drop or the Vulkan HDR10+ parse turns on after submitDirectDecodeUnit checked, the
 * frame must get its array copy before anything indexes it.
 */
public class DirectDecodeUnitArrayTest {
    @Test
    public void framesJavaReadsNeedAnArray() {
        assertFalse(MediaCodecDecoderRenderer.needsArray(false, false, false));
        assertTrue(MediaCodecDecoderRenderer.needsArray(true, false, false));
        assertTrue(MediaCodecDecoderRenderer.needsArray(false, true, false));
        assertTrue(MediaCodecDecoderRenderer.needsArray(false, false, true));
    }

    @Test
    public void directArrayCopiesTheFrameAndReusesABigEnoughArray() {
        ByteBuffer direct = ByteBuffer.allocateDirect(8);
        direct.put(new byte[]{0, 0, 1, 0x7c, 1, 2, 3, 4});
        // The caller's position must not matter or change.
        int position = direct.position();

        byte[] first = MediaCodecDecoderRenderer.directArray(direct, 6, null);
        assertEquals(6, first.length);
        assertArrayEquals(new byte[]{0, 0, 1, 0x7c, 1, 2}, first);
        assertEquals(position, direct.position());

        byte[] reused = MediaCodecDecoderRenderer.directArray(direct, 4, first);
        assertSame(first, reused);
        assertArrayEquals(new byte[]{0, 0, 1, 0x7c}, java.util.Arrays.copyOf(reused, 4));

        byte[] grown = MediaCodecDecoderRenderer.directArray(direct, 8, first);
        assertNotSame(first, grown);
        assertArrayEquals(new byte[]{0, 0, 1, 0x7c, 1, 2, 3, 4}, grown);
    }

    @Test
    public void submitDecodeUnitGivesADirectFrameItsArrayBeforeReadingIt() throws Exception {
        String source = new String(Files.readAllBytes(
                Paths.get("src/main/java/com/limelight/binding/video/MediaCodecDecoderRenderer.java")), StandardCharsets.UTF_8);
        // submitDecodeUnit wraps queueDecodeUnit for the submit-thread hint; the body lives there.
        int submit = source.indexOf("private int queueDecodeUnit(byte[] decodeUnitData");
        int end = source.indexOf("holdForInputSpacing();", submit);
        assertTrue(submit >= 0 && end > submit);
        String body = source.substring(submit, end);

        int guard = body.indexOf("if (decodeUnitData == null && needsArray(");
        assertTrue("submitDecodeUnit lacks the array guard for direct frames", guard >= 0);
        // The picture-data reads of the array all come after the guard.
        for (String read : new String[]{"HdrBitstream.queuedLength(", "HdrBitstream.tail(",
                "HdrBitstream.findVividSei(", "HdrBitstream.put(", "noteVulkanHdr10Plus("}) {
            int at = body.indexOf(read);
            assertTrue(read + " missing", at >= 0);
            assertTrue(read + " reads the array before the guard", at > guard);
        }
        // The HDR10+ parse uses the same decision as the guard, not a fresh read of the fields.
        assertFalse(body.contains("if (vulkanHdr10Plus && Boolean.TRUE.equals(currentHdrMode))"));
    }
}

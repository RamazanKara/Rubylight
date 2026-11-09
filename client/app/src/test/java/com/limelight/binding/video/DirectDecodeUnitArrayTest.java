package com.limelight.binding.video;

import org.junit.Test;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Paths;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

/**
 * A frame handed over in place reaches submitDecodeUnit without an array. Dolby Vision's trim, the
 * HDR Vivid SEI drop and the Vulkan HDR10+ parse read it where it is, even when one of them turns on
 * after submitDirectDecodeUnit routed the frame, so no array copy of the picture is made.
 */
public class DirectDecodeUnitArrayTest {
    private static String source() throws Exception {
        return new String(Files.readAllBytes(
                Paths.get("src/main/java/com/limelight/binding/video/MediaCodecDecoderRenderer.java")), StandardCharsets.UTF_8);
    }

    private static String between(String source, String from, String to) {
        int start = source.indexOf(from);
        int end = source.indexOf(to, start);
        assertTrue(from + " missing", start >= 0 && end > start);
        return source.substring(start, end);
    }

    @Test
    public void queueDecodeUnitReadsADirectFrameInPlace() throws Exception {
        // submitDecodeUnit wraps queueDecodeUnit for the submit-thread hint; the body lives there.
        String body = between(source(), "private int queueDecodeUnit(byte[] decodeUnitData", "holdForInputSpacing();");
        assertFalse("a direct frame is copied into an array", body.contains("directArray"));
        for (String read : new String[]{"HdrBitstream.queuedLength(dolbyVision, direct,", "HdrBitstream.tail(direct,",
                "HdrBitstream.findVividSei(direct,", "HdrBitstream.put(nextInputBuffer, direct,",
                "noteVulkanHdr10Plus(vulkan, timestampUs, direct != null ? direct"}) {
            assertTrue(read + " missing", body.contains(read));
        }
        // The HDR10+ parse uses the same decision as the rest of the frame, not a fresh read of the fields.
        assertFalse(body.contains("if (vulkanHdr10Plus && Boolean.TRUE.equals(currentHdrMode))"));
    }

    @Test
    public void onlyParameterSetsTakeTheArrayCopy() throws Exception {
        String body = between(source(), "public int submitDirectDecodeUnit(", "directInput = decodeUnitData;");
        assertTrue(body.contains("if (decodeUnitType != MoonBridge.BUFFER_TYPE_PICDATA) {"));
        for (String field : new String[]{"dolbyVisionActive", "vividSeiDropDecoder", "vulkanHdr10Plus"}) {
            assertFalse(field + " sends picture data to the array copy", body.contains(field));
        }
    }
}

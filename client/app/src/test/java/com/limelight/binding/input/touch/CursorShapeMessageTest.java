package com.limelight.binding.input.touch;

import com.limelight.binding.input.touch.CursorShapeMessage.Kind;
import com.limelight.binding.input.touch.CursorShapeMessage.MalformedException;

import org.junit.Test;

import java.util.Arrays;

import static org.junit.Assert.*;

public class CursorShapeMessageTest {
    private static byte[] header(int version, int kind, int id) {
        return new byte[] {(byte) version, (byte) kind, 0, 0, (byte) id, (byte) (id >> 8), (byte) (id >> 16), (byte) (id >> 24)};
    }

    /** An IMAGE whose pixel i is RGBA (i, i + 1, i + 2, 255 - i). */
    private static byte[] image(int id, int width, int height, int hotspotX, int hotspotY, int format, int scalePercent) {
        byte[] message = new byte[CursorShapeMessage.HEADER_BYTES + CursorShapeMessage.IMAGE_HEADER_BYTES + width * height * 4];
        System.arraycopy(header(1, 3, id), 0, message, 0, CursorShapeMessage.HEADER_BYTES);
        int[] fields = {width, height, hotspotX, hotspotY};
        for (int i = 0; i < fields.length; i++) {
            message[8 + i * 2] = (byte) fields[i];
            message[9 + i * 2] = (byte) (fields[i] >> 8);
        }
        message[16] = (byte) format;
        message[18] = (byte) scalePercent;
        message[19] = (byte) (scalePercent >> 8);
        for (int i = 0; i < width * height; i++) {
            message[20 + i * 4] = (byte) i;
            message[21 + i * 4] = (byte) (i + 1);
            message[22 + i * 4] = (byte) (i + 2);
            message[23 + i * 4] = (byte) (255 - i);
        }
        return message;
    }

    private static byte[] image(int width, int height) {
        return image(7, width, height, 0, 0, CursorShapeMessage.FORMAT_RGBA8888, 100);
    }

    private static void assertRejected(String expected, byte[] message) {
        try {
            CursorShapeMessage.parse(message);
            fail("accepted: " + expected);
        } catch (MalformedException e) {
            assertTrue(e.getMessage(), e.getMessage().contains(expected));
        }
    }

    @Test
    public void hideDefaultAndShowCarryOnlyTheHeader() throws MalformedException {
        assertEquals(Kind.HIDE, CursorShapeMessage.parse(header(1, 0, 0)).kind);
        assertEquals(Kind.DEFAULT, CursorShapeMessage.parse(header(1, 1, 0)).kind);
        CursorShapeMessage show = CursorShapeMessage.parse(header(1, 2, 0xFEDCBA98));
        assertEquals(Kind.SHOW, show.kind);
        assertEquals(0xFEDCBA98, show.id);
    }

    @Test
    public void imageCarriesSizeHotspotScaleAndUnpremultipliedArgb() throws MalformedException {
        CursorShapeMessage message = CursorShapeMessage.parse(image(0x01020304, 3, 2, 2, 1, 1, 250));
        assertEquals(Kind.IMAGE, message.kind);
        assertEquals(0x01020304, message.id);
        assertEquals(3, message.width);
        assertEquals(2, message.height);
        assertEquals(2, message.hotspotX);
        assertEquals(1, message.hotspotY);
        assertEquals(250, message.scalePercent);
        int[] pixels = message.argbPixels();
        assertEquals(6, pixels.length);
        assertEquals(0xFF000102, pixels[0]);
        assertEquals(0xFA050607, pixels[5]);
    }

    @Test
    public void largestImageIsAccepted() throws MalformedException {
        byte[] largest = image(CursorShapeMessage.MAX_EDGE, CursorShapeMessage.MAX_EDGE);
        assertEquals(CursorShapeMessage.MAX_BYTES, largest.length);
        assertEquals(CursorShapeMessage.MAX_EDGE * CursorShapeMessage.MAX_EDGE, CursorShapeMessage.parse(largest).argbPixels().length);
        assertEquals(1, CursorShapeMessage.parse(image(1, 1)).argbPixels().length);
        assertEquals(400, CursorShapeMessage.parse(image(7, 2, 2, 1, 1, 1, 400)).scalePercent);
    }

    @Test
    public void malformedHeadersAreRejected() {
        assertRejected("too short", null);
        assertRejected("too short", new byte[0]);
        assertRejected("too short", Arrays.copyOf(header(1, 0, 0), 7));
        assertRejected("too large", new byte[CursorShapeMessage.MAX_BYTES + 1]);
        assertRejected("version 0", header(0, 0, 0));
        assertRejected("version 2", header(2, 0, 0));
        assertRejected("unknown kind 4", header(1, 4, 0));
        assertRejected("unknown kind 255", header(1, 255, 0));
        byte[] reserved = header(1, 0, 0);
        reserved[3] = 1;
        assertRejected("reserved header", reserved);
        assertRejected("HIDE with id", header(1, 0, 5));
        assertRejected("DEFAULT with id", header(1, 1, 5));
        assertRejected("SHOW with id 0", header(1, 2, 0));
        assertRejected("SHOW has 9 bytes", Arrays.copyOf(header(1, 2, 5), 9));
        assertRejected("HIDE has 20 bytes", Arrays.copyOf(header(1, 0, 0), 20));
    }

    @Test
    public void malformedImagesAreRejected() {
        assertRejected("IMAGE with id 0", image(0, 2, 2, 0, 0, 1, 100));
        assertRejected("cut short", Arrays.copyOf(image(2, 2), 19));
        assertRejected("cut short", header(1, 3, 7));
        assertRejected("size 0x2", image(7, 0, 2, 0, 0, 1, 100));
        assertRejected("size 2x0", image(7, 2, 0, 0, 0, 1, 100));
        assertRejected("size 97x1", image(7, 97, 1, 0, 0, 1, 100));
        assertRejected("hotspot 2,0", image(7, 2, 2, 2, 0, 1, 100));
        assertRejected("hotspot 0,2", image(7, 2, 2, 0, 2, 1, 100));
        assertRejected("pixel format 0", image(7, 2, 2, 0, 0, 0, 100));
        assertRejected("pixel format 2", image(7, 2, 2, 0, 0, 2, 100));
        assertRejected("scale 99%", image(7, 2, 2, 0, 0, 1, 99));
        assertRejected("scale 401%", image(7, 2, 2, 0, 0, 1, 401));
        byte[] reserved = image(2, 2);
        reserved[17] = 1;
        assertRejected("reserved image byte", reserved);
        byte[] full = image(2, 2);
        assertRejected("has 35 bytes, expected 36", Arrays.copyOf(full, full.length - 1));
        assertRejected("has 37 bytes, expected 36", Arrays.copyOf(full, full.length + 1));
        // A size field that claims more pixels than were sent.
        byte[] lying = image(2, 2);
        lying[8] = 96;
        assertRejected("96x2 image has 36 bytes", lying);
    }

    @Test(expected = IllegalStateException.class)
    public void onlyImagesHavePixels() throws MalformedException {
        CursorShapeMessage.parse(header(1, 0, 0)).argbPixels();
    }
}

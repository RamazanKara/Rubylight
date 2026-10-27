package com.limelight.binding.input.touch;

import java.util.Locale;

/**
 * One host cursor shape message (control message 0x5541, docs/protocol/cursor-shape.md), checked
 * against every bound of the format. Anything outside them is rejected with the reason, so the
 * caller can log it and fall back to the default arrow.
 */
public final class CursorShapeMessage {
    public static final int VERSION = 1;
    public static final int MAX_EDGE = 96;
    public static final int HEADER_BYTES = 8;
    public static final int IMAGE_HEADER_BYTES = 12;
    public static final int MAX_BYTES = HEADER_BYTES + IMAGE_HEADER_BYTES + MAX_EDGE * MAX_EDGE * 4;
    public static final int FORMAT_RGBA8888 = 1;
    public static final int MIN_SCALE_PERCENT = 100;
    public static final int MAX_SCALE_PERCENT = 400;

    /** In wire order: kind 0 hides the pointer, 1 draws the default arrow, 2 a cached image, 3 a new one. */
    public enum Kind { HIDE, DEFAULT, SHOW, IMAGE }

    public static final class MalformedException extends Exception {
        MalformedException(String message) {
            super(message);
        }
    }

    public final Kind kind;
    /** Cache id, unsigned 32-bit; 0 for HIDE and DEFAULT. */
    public final int id;
    /** IMAGE only: size and hotspot in host pixels, and the host's display scale in percent. */
    public final int width, height, hotspotX, hotspotY, scalePercent;
    private final byte[] message;

    private CursorShapeMessage(Kind kind, int id, int width, int height, int hotspotX, int hotspotY,
                               int scalePercent, byte[] message) {
        this.kind = kind;
        this.id = id;
        this.width = width;
        this.height = height;
        this.hotspotX = hotspotX;
        this.hotspotY = hotspotY;
        this.scalePercent = scalePercent;
        this.message = message;
    }

    public static CursorShapeMessage parse(byte[] message) throws MalformedException {
        if (message == null || message.length < HEADER_BYTES) {
            throw new MalformedException("too short: " + (message == null ? 0 : message.length) + " bytes");
        }
        if (message.length > MAX_BYTES) {
            throw new MalformedException("too large: " + message.length + " bytes, at most " + MAX_BYTES);
        }
        if (u8(message, 0) != VERSION) {
            throw new MalformedException("unsupported version " + u8(message, 0));
        }
        int kindValue = u8(message, 1);
        if (kindValue >= Kind.values().length) {
            throw new MalformedException("unknown kind " + kindValue);
        }
        if (u16(message, 2) != 0) {
            throw new MalformedException("reserved header bits set");
        }
        Kind kind = Kind.values()[kindValue];
        int id = u32(message, 4);
        if (kind != Kind.IMAGE) {
            if (message.length != HEADER_BYTES) {
                throw new MalformedException(kind + " has " + message.length + " bytes, expected " + HEADER_BYTES);
            }
            if ((kind == Kind.SHOW) == (id == 0)) {
                throw new MalformedException(kind + " with id " + (id & 0xFFFFFFFFL));
            }
            return new CursorShapeMessage(kind, id, 0, 0, 0, 0, 0, null);
        }
        if (id == 0) {
            throw new MalformedException("IMAGE with id 0");
        }
        if (message.length < HEADER_BYTES + IMAGE_HEADER_BYTES) {
            throw new MalformedException("IMAGE header cut short at " + message.length + " bytes");
        }
        int offset = HEADER_BYTES;
        int width = u16(message, offset), height = u16(message, offset + 2);
        int hotspotX = u16(message, offset + 4), hotspotY = u16(message, offset + 6);
        int format = u8(message, offset + 8);
        int reserved = u8(message, offset + 9);
        int scalePercent = u16(message, offset + 10);
        if (width < 1 || width > MAX_EDGE || height < 1 || height > MAX_EDGE) {
            throw new MalformedException(String.format(Locale.ROOT, "size %dx%d outside 1-%d", width, height, MAX_EDGE));
        }
        if (hotspotX >= width || hotspotY >= height) {
            throw new MalformedException(String.format(Locale.ROOT, "hotspot %d,%d outside %dx%d",
                    hotspotX, hotspotY, width, height));
        }
        if (format != FORMAT_RGBA8888) {
            throw new MalformedException("unsupported pixel format " + format);
        }
        if (reserved != 0) {
            throw new MalformedException("reserved image byte set");
        }
        if (scalePercent < MIN_SCALE_PERCENT || scalePercent > MAX_SCALE_PERCENT) {
            throw new MalformedException("scale " + scalePercent + "% outside " + MIN_SCALE_PERCENT + "-" +
                    MAX_SCALE_PERCENT + "%");
        }
        int expected = HEADER_BYTES + IMAGE_HEADER_BYTES + width * height * 4;
        if (message.length != expected) {
            throw new MalformedException(String.format(Locale.ROOT, "%dx%d image has %d bytes, expected %d",
                    width, height, message.length, expected));
        }
        return new CursorShapeMessage(kind, id, width, height, hotspotX, hotspotY, scalePercent, message);
    }

    /** IMAGE only: the pixels as unpremultiplied ARGB, row by row from the top. */
    public int[] argbPixels() {
        if (kind != Kind.IMAGE) {
            throw new IllegalStateException(kind + " carries no pixels");
        }
        int[] pixels = new int[width * height];
        int offset = HEADER_BYTES + IMAGE_HEADER_BYTES;
        for (int i = 0; i < pixels.length; i++, offset += 4) {
            pixels[i] = u8(message, offset + 3) << 24 | u8(message, offset) << 16 |
                    u8(message, offset + 1) << 8 | u8(message, offset + 2);
        }
        return pixels;
    }

    private static int u8(byte[] bytes, int offset) {
        return bytes[offset] & 0xFF;
    }

    private static int u16(byte[] bytes, int offset) {
        return u8(bytes, offset) | u8(bytes, offset + 1) << 8;
    }

    private static int u32(byte[] bytes, int offset) {
        return u16(bytes, offset) | u16(bytes, offset + 2) << 16;
    }
}

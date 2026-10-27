package com.limelight.nvstream.http;

import java.io.IOException;
import java.util.Arrays;

import okhttp3.MediaType;
import okhttp3.ResponseBody;

/**
 * Why the image clipboard ({@code /actions/clipboard?type=png}) gave no image or took none, with the host's limits
 * the PNG is checked against before it is sent and after it arrives, as nightjar-core's clipboard_image.rs does.
 */
public final class ClipboardImageException extends IOException {
    public enum Reason {
        /** Over the byte or pixel limits, found here or the host's 413 (its own byte limit may be lower). */
        TOO_LARGE,
        /** Not a PNG or a broken one, found here or the host's 400. */
        NOT_PNG,
        /** 415: the host can't convert the image on its clipboard, or decode the one sent. */
        UNSUPPORTED,
        /** 403: the image clipboard is off, the device lacks the clipboard permission or has no stream of its own. */
        FORBIDDEN,
    }

    /** The most bytes an image may have: the top of the host's clipboard_image_max_mb (1 to 64, 16 by default). */
    public static final int MAX_BYTES = 64 * 1024 * 1024;
    /** The widest and tallest image the host decodes. */
    public static final int MAX_SIDE = 16384;
    /** The most pixels the host decodes: 32 megapixels. */
    public static final long MAX_PIXELS = 1L << 25;
    static final byte[] PNG_SIGNATURE = {(byte) 0x89, 'P', 'N', 'G', '\r', '\n', 0x1a, '\n'};
    private static final byte[] IHDR = {0, 0, 0, 13, 'I', 'H', 'D', 'R'};

    public final Reason reason;

    public ClipboardImageException(Reason reason, String message) {
        super(message);
        this.reason = reason;
    }

    /** Whether {@code bytes} start with the PNG signature; {@link #pngSize} checks the rest. */
    public static boolean isPng(byte[] bytes) {
        return bytes.length >= PNG_SIGNATURE.length && Arrays.equals(Arrays.copyOf(bytes, PNG_SIGNATURE.length), PNG_SIGNATURE);
    }

    /** The width and height a PNG's header gives, once its signature, IHDR chunk and the host's limits check out. */
    public static int[] pngSize(byte[] png) throws ClipboardImageException {
        if (png.length > MAX_BYTES) {
            throw new ClipboardImageException(Reason.TOO_LARGE, "The image has " + png.length + " bytes, over " + MAX_BYTES);
        }
        if (png.length < 24 || !Arrays.equals(Arrays.copyOf(png, 8), PNG_SIGNATURE)) {
            throw new ClipboardImageException(Reason.NOT_PNG, "Not a PNG image");
        }
        if (!Arrays.equals(Arrays.copyOfRange(png, 8, 16), IHDR)) {
            throw new ClipboardImageException(Reason.NOT_PNG, "The PNG has no IHDR header");
        }
        long width = readUnsigned(png, 16), height = readUnsigned(png, 20);
        if (width == 0 || height == 0) {
            throw new ClipboardImageException(Reason.NOT_PNG, "An empty " + width + "x" + height + " image");
        }
        if (width > MAX_SIDE || height > MAX_SIDE || width * height > MAX_PIXELS) {
            throw new ClipboardImageException(Reason.TOO_LARGE, "The image is " + width + "x" + height + ", over "
                    + MAX_SIDE + " a side or " + MAX_PIXELS + " pixels");
        }
        return new int[] {(int) width, (int) height};
    }

    private static long readUnsigned(byte[] bytes, int at) {
        return ((bytes[at] & 0xffL) << 24) | ((bytes[at + 1] & 0xffL) << 16) | ((bytes[at + 2] & 0xffL) << 8)
                | (bytes[at + 3] & 0xffL);
    }

    /** Throws the error a non-2xx status of the route stands for; 404 on a read is handled before this. */
    static void check(int status, String message) throws IOException {
        switch (status) {
            case 200: return;
            case 400: throw new ClipboardImageException(Reason.NOT_PNG, "The PC could not read the image");
            case 403: throw new ClipboardImageException(Reason.FORBIDDEN, "The PC refused its clipboard to this device");
            case 413: throw new ClipboardImageException(Reason.TOO_LARGE, "The image is over the PC's limit");
            case 415: throw new ClipboardImageException(Reason.UNSUPPORTED, "The PC can't convert this image");
            default: throw new HostHttpResponseException(status, message);
        }
    }

    /** A 200 answer's PNG: of type image/png, within the limits and with a PNG header. */
    static byte[] readPng(ResponseBody body) throws IOException {
        MediaType type = body.contentType();
        if (type == null || !type.type().equalsIgnoreCase("image") || !type.subtype().equalsIgnoreCase("png")) {
            throw new IOException("The PC sent " + type + " for a clipboard image");
        }
        if (body.source().request(MAX_BYTES + 1L)) {
            throw new ClipboardImageException(Reason.TOO_LARGE, "The PC's image is over " + MAX_BYTES + " bytes");
        }
        byte[] png = body.bytes();
        pngSize(png);
        return png;
    }
}

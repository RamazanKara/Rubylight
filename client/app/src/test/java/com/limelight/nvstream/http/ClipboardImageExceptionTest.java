package com.limelight.nvstream.http;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.ByteBuffer;
import java.nio.file.Files;

import okhttp3.MediaType;
import okhttp3.ResponseBody;
import okio.Okio;

/** The host's image clipboard limits, checked on both sides of the transfer as nightjar-core checks them. */
public class ClipboardImageExceptionTest {
    /** A PNG signature and IHDR chunk of the given size; enough for the header checks. */
    static byte[] png(long width, long height) {
        ByteBuffer png = ByteBuffer.allocate(33);
        png.put(ClipboardImageException.PNG_SIGNATURE);
        png.putInt(13).put(new byte[] {'I', 'H', 'D', 'R'});
        png.putInt((int) width).putInt((int) height);
        return png.array();
    }

    private static ClipboardImageException.Reason reason(byte[] png) {
        return assertThrows(ClipboardImageException.class, () -> ClipboardImageException.pngSize(png)).reason;
    }

    @Test
    public void aPngWithinTheHostLimitsGivesItsSize() throws IOException {
        assertArrayEquals(new int[] {1920, 1080}, ClipboardImageException.pngSize(png(1920, 1080)));
        assertArrayEquals(new int[] {16384, 2048}, ClipboardImageException.pngSize(png(16384, 2048)));
        assertTrue(ClipboardImageException.isPng(png(1, 1)));
    }

    @Test
    public void anImageOverASideOrThePixelLimitIsTooLarge() {
        assertEquals(ClipboardImageException.Reason.TOO_LARGE, reason(png(16385, 1)));
        assertEquals(ClipboardImageException.Reason.TOO_LARGE, reason(png(1, 16385)));
        // 8192 x 8192 is 64 megapixels: within each side, over the 32 megapixels.
        assertEquals(ClipboardImageException.Reason.TOO_LARGE, reason(png(8192, 8192)));
        // The width is unsigned: 0xffffffff is not -1.
        assertEquals(ClipboardImageException.Reason.TOO_LARGE, reason(png(0xffffffffL, 1)));
    }

    @Test
    public void anythingButAPngHeaderIsNotAPng() {
        assertEquals(ClipboardImageException.Reason.NOT_PNG, reason(png(0, 10)));
        byte[] jpeg = png(10, 10);
        jpeg[1] = 'J';
        assertEquals(ClipboardImageException.Reason.NOT_PNG, reason(jpeg));
        assertFalse(ClipboardImageException.isPng(jpeg));
        byte[] noHeader = png(10, 10);
        noHeader[12] = 'X';
        assertEquals(ClipboardImageException.Reason.NOT_PNG, reason(noHeader));
        assertEquals(ClipboardImageException.Reason.NOT_PNG, reason(ClipboardImageException.PNG_SIGNATURE));
        assertFalse(ClipboardImageException.isPng(new byte[3]));
    }

    @Test
    public void eachDocumentedStatusIsItsReasonAndOthersStayHttpErrors() throws IOException {
        ClipboardImageException.check(200, "OK");
        int[] statuses = {400, 403, 413, 415};
        ClipboardImageException.Reason[] reasons = {ClipboardImageException.Reason.NOT_PNG,
                ClipboardImageException.Reason.FORBIDDEN, ClipboardImageException.Reason.TOO_LARGE,
                ClipboardImageException.Reason.UNSUPPORTED};
        for (int i = 0; i < statuses.length; i++) {
            int status = statuses[i];
            assertEquals(reasons[i], assertThrows(ClipboardImageException.class,
                    () -> ClipboardImageException.check(status, "x")).reason);
        }
        HostHttpResponseException other = assertThrows(HostHttpResponseException.class,
                () -> ClipboardImageException.check(500, "Server Error"));
        assertEquals(500, other.getErrorCode());
    }

    private static byte[] written(ResponseBody body) throws IOException {
        ByteArrayOutputStream out = new ByteArrayOutputStream();
        assertEquals(body.contentLength(), ClipboardImageException.writePng(body, out));
        return out.toByteArray();
    }

    @Test
    public void aReceivedImageMustBeAPngOfTypeImagePng() throws IOException {
        byte[] image = png(4, 4);
        assertArrayEquals(image, written(ResponseBody.create(image, MediaType.get("image/png"))));
        assertThrows(IOException.class, () -> written(
                ResponseBody.create(image, MediaType.get("application/octet-stream"))));
        assertThrows(IOException.class, () -> written(ResponseBody.create(image, null)));
        assertEquals(ClipboardImageException.Reason.NOT_PNG, assertThrows(ClipboardImageException.class,
                () -> written(ResponseBody.create("<html>", MediaType.get("image/png")))).reason);
        assertEquals(ClipboardImageException.Reason.TOO_LARGE, assertThrows(ClipboardImageException.class,
                () -> written(ResponseBody.create(new byte[ClipboardImageException.MAX_BYTES + 1],
                        MediaType.get("image/png")))).reason);
    }

    /** A PNG header followed by zeros, {@code length} bytes in all, made as it is read. */
    private static InputStream endless(long length) {
        byte[] head = png(4, 4);
        return new InputStream() {
            long at;
            @Override public int read() {
                byte[] one = new byte[1];
                return read(one, 0, 1) == -1 ? -1 : one[0] & 0xff;
            }
            @Override public int read(byte[] buffer, int offset, int count) {
                if (at >= length) return -1;
                int n = (int) Math.min(count, length - at);
                for (int i = 0; i < n; i++) buffer[offset + i] = at + i < head.length ? head[(int) (at + i)] : 0;
                at += n;
                return n;
            }
        };
    }

    @Test
    public void aReceivedImageOfUnknownLengthIsStreamedAndCutAtTheLimit() throws IOException {
        long[] kept = {0};
        OutputStream counting = new OutputStream() {
            @Override public void write(int b) { kept[0]++; }
            @Override public void write(byte[] b, int off, int len) { kept[0] += len; }
        };
        ResponseBody chunked = ResponseBody.create(Okio.buffer(Okio.source(endless(ClipboardImageException.MAX_BYTES + 1L))),
                MediaType.get("image/png"), -1);
        assertEquals(ClipboardImageException.Reason.TOO_LARGE, assertThrows(ClipboardImageException.class,
                () -> ClipboardImageException.writePng(chunked, counting)).reason);
        assertTrue("wrote " + kept[0], kept[0] <= ClipboardImageException.MAX_BYTES);
        ResponseBody fits = ResponseBody.create(Okio.buffer(Okio.source(endless(ClipboardImageException.MAX_BYTES))),
                MediaType.get("image/png"), -1);
        kept[0] = 0;
        assertEquals(ClipboardImageException.MAX_BYTES, ClipboardImageException.writePng(fits, counting));
        assertEquals(ClipboardImageException.MAX_BYTES, kept[0]);
    }

    @Test
    public void aFileIsCheckedByItsHeaderAndLength() throws IOException {
        File file = File.createTempFile("clipboard", ".png");
        try {
            Files.write(file.toPath(), png(64, 32));
            assertArrayEquals(new int[] {64, 32}, ClipboardImageException.pngSize(file));
            Files.write(file.toPath(), png(16385, 1));
            assertEquals(ClipboardImageException.Reason.TOO_LARGE, assertThrows(ClipboardImageException.class,
                    () -> ClipboardImageException.pngSize(file)).reason);
            Files.write(file.toPath(), new byte[] {1, 2, 3});
            assertEquals(ClipboardImageException.Reason.NOT_PNG, assertThrows(ClipboardImageException.class,
                    () -> ClipboardImageException.pngSize(file)).reason);
        } finally {
            assertTrue(file.delete());
        }
    }
}

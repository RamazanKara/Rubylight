package com.limelight.utils;

import org.junit.Test;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.atomic.AtomicBoolean;

import static org.junit.Assert.*;

public class CacheHelperTest {
    @Test
    public void exactSizeLimitIsAccepted() throws Exception {
        byte[] bytes = new byte[8192];
        ByteArrayOutputStream output = new ByteArrayOutputStream();
        CacheHelper.writeInputStreamToOutputStream(new ByteArrayInputStream(bytes), output, bytes.length);
        assertArrayEquals(bytes, output.toByteArray());
    }

    @Test
    public void exceedingTheSizeLimitStillFails() {
        assertThrows(IOException.class, () -> CacheHelper.writeInputStreamToOutputStream(
                new ByteArrayInputStream(new byte[8193]), new ByteArrayOutputStream(), 8192));
    }

    @Test
    public void failedCacheReadClosesItsInput() {
        AtomicBoolean closed = new AtomicBoolean();
        InputStream input = new InputStream() {
            @Override public int read() throws IOException { throw new IOException("Read failed"); }
            @Override public void close() { closed.set(true); }
        };
        assertThrows(IOException.class, () -> CacheHelper.readInputStreamToString(input));
        assertTrue(closed.get());
    }

    @Test
    public void cachedTextRoundTripsUtf8() throws Exception {
        String text = "Spiele – 日本語";
        ByteArrayOutputStream output = new ByteArrayOutputStream();
        CacheHelper.writeStringToOutputStream(output, text);
        assertArrayEquals(text.getBytes(StandardCharsets.UTF_8), output.toByteArray());
        assertEquals(text, CacheHelper.readInputStreamToString(new ByteArrayInputStream(output.toByteArray())));
    }

    @Test
    public void closeFailureDoesNotDiscardCachedText() throws Exception {
        InputStream input = new ByteArrayInputStream("cached".getBytes(StandardCharsets.UTF_8)) {
            @Override public void close() throws IOException { throw new IOException("Close failed"); }
        };
        assertEquals("cached", CacheHelper.readInputStreamToString(input));
    }
}

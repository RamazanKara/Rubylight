package com.limelight.utils;

import org.junit.Test;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;

import static org.junit.Assert.*;

public class NativeCrashTraceTest {
    @Test
    public void protobufTraceRetainsSignalAndFramesButNotFreeTextOrMemory() throws Exception {
        byte[] frame = concat(number(1, 0x123), number(2, 0xdeadbeefL),
                string(4, "secret symbol"), string(6, "/data/private-host/libmoonlight-core.so"));
        byte[] thread = concat(number(1, 42), string(2, "desktop.local"), field(4, frame),
                field(5, string(4, "password hunter2")), string(7, "PIN 1234"));
        byte[] trace = concat(number(6, 42), string(9, "https://user:secret@desktop.local"),
                field(10, concat(number(1, 6), number(3, 0), number(9, 0xdeadbeefL))),
                string(14, "Abort: 192.168.1.2 fe80::1234"),
                field(16, concat(number(1, 42), field(2, thread))),
                field(18, string(1, "arbitrary secret log")));
        String text = RedactedLog.nativeTrace(new ByteArrayInputStream(trace));
        assertTrue(text.contains("Crashing thread ID: 42"));
        assertTrue(text.contains("Signal: 6"));
        assertTrue(text.contains("Signal code: 0"));
        assertTrue(text.contains("pc 123 libmoonlight-core.so"));
        for (String secret : new String[] {"private-host", "desktop.local", "secret", "hunter2", "PIN", "1234", "deadbeef", "192.168", "fe80"}) {
            assertFalse(secret, text.contains(secret));
        }
    }

    @Test
    public void rustEngineFramesAreKeptLikeTheCEngineOnes() throws Exception {
        byte[] frame = concat(number(1, 0x77), string(6, "/data/app/~~x/lib/arm64/libnightjar_core.so"));
        byte[] trace = field(16, field(2, field(4, frame)));
        String text = RedactedLog.nativeTrace(new ByteArrayInputStream(trace));
        assertTrue(text.contains("pc 77 libnightjar_core.so"));
        assertFalse(text.contains("~~x"));
    }

    @Test
    public void truncatedProtobufKeepsTheSafePrefixAndNeverReturnsRawBytes() throws Exception {
        byte[] trace = concat(field(10, number(1, 11)), string(14, "secret-host"));
        String text = RedactedLog.nativeTrace(new ByteArrayInputStream(Arrays.copyOf(trace, trace.length - 3)));
        assertTrue(text.contains("Signal: 11"));
        assertFalse(text.contains("secret"));
        assertFalse(RedactedLog.nativeTrace(new ByteArrayInputStream("Abort: secret".getBytes(StandardCharsets.UTF_8))).contains("secret"));
    }

    @Test
    public void nativeTraceBoundsBothInputReadAndOutput() throws Exception {
        ByteArrayOutputStream frames = new ByteArrayOutputStream();
        for (int i = 0; i < 10000; i++) {
            frames.write(field(4, concat(number(1, i), string(6, "/system/lib64/libc.so"))));
        }
        byte[] trace = field(16, field(2, frames.toByteArray()));
        ByteArrayInputStream input = new ByteArrayInputStream(trace);
        String text = RedactedLog.nativeTrace(input);
        assertEquals(RedactedLog.MAX_TRACE_BYTES, trace.length - input.available());
        assertTrue(text.length() <= RedactedLog.MAX_TRACE_CHARS);
        assertTrue(text.contains("pc 0 libc.so"));
    }

    @Test
    public void unknownLibrariesAndMalformedLengthsCannotLeakText() throws Exception {
        byte[] trace = field(16, field(2, field(4, concat(number(1, 10), string(6, "/private/desktop.local")))));
        String text = RedactedLog.nativeTrace(new ByteArrayInputStream(trace));
        assertTrue(text.contains("pc a [library omitted]"));
        assertFalse(text.contains("desktop.local"));
        byte[] malformed = concat(number(10, 3), new byte[] {0x72, (byte) 0xff, (byte) 0xff, (byte) 0xff, (byte) 0xff, 0x7f});
        assertTrue(RedactedLog.nativeTrace(new ByteArrayInputStream(malformed)).contains("trace incomplete"));
    }

    private static byte[] string(int field, String value) throws IOException {
        return field(field, value.getBytes(StandardCharsets.UTF_8));
    }

    private static byte[] field(int field, byte[] value) throws IOException {
        return concat(varint((field << 3) | 2), varint(value.length), value);
    }

    private static byte[] number(int field, long value) throws IOException {
        return concat(varint(field << 3), varint(value));
    }

    private static byte[] varint(long value) {
        ByteArrayOutputStream output = new ByteArrayOutputStream();
        while ((value & ~0x7fL) != 0) {
            output.write((int) (value & 0x7f) | 0x80);
            value >>>= 7;
        }
        output.write((int) value);
        return output.toByteArray();
    }

    private static byte[] concat(byte[]... chunks) throws IOException {
        ByteArrayOutputStream output = new ByteArrayOutputStream();
        for (byte[] chunk : chunks) {
            output.write(chunk);
        }
        return output.toByteArray();
    }
}

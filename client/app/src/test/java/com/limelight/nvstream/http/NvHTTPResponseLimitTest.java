package com.limelight.nvstream.http;

import org.junit.Test;

import java.io.IOException;
import java.util.Arrays;

import okhttp3.MediaType;
import okhttp3.ResponseBody;
import okio.Buffer;
import okio.BufferedSource;

import static org.junit.Assert.*;

public class NvHTTPResponseLimitTest {
    private static ResponseBody body(int size, long declaredLength) {
        byte[] bytes = new byte[size];
        Arrays.fill(bytes, (byte) 'a');
        BufferedSource source = new Buffer().write(bytes);
        return new ResponseBody() {
            @Override public MediaType contentType() { return MediaType.parse("text/xml; charset=utf-8"); }
            @Override public long contentLength() { return declaredLength; }
            @Override public BufferedSource source() { return source; }
        };
    }

    @Test
    public void rejectsOversizedDeclaredAndChunkedDiscoveryResponses() {
        int limit = NvHTTP.MAX_XML_RESPONSE_BYTES;
        for (long length : new long[] {limit + 1L, -1, 1}) {
            try (ResponseBody response = body(limit + 1, length)) {
                assertThrows(IOException.class, () -> NvHTTP.readXmlResponse(response, "serverinfo"));
            }
        }
    }

    @Test
    public void acceptsBoundaryAndAllowsLargerBoundedAppLists() throws Exception {
        int limit = NvHTTP.MAX_XML_RESPONSE_BYTES;
        try (ResponseBody response = body(limit, -1)) {
            assertEquals(limit, NvHTTP.readXmlResponse(response, "serverinfo").length());
        }
        try (ResponseBody response = body(limit + 1, -1)) {
            assertEquals(limit + 1, NvHTTP.readXmlResponse(response, "applist").length());
        }
        try (ResponseBody response = body(NvHTTP.MAX_APP_LIST_BYTES + 1, -1)) {
            assertThrows(IOException.class, () -> NvHTTP.readXmlResponse(response, "applist"));
        }
    }
}

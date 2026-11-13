package com.limelight.nvstream.http;

import org.junit.Test;

import java.io.StringReader;
import java.util.LinkedList;

import static org.junit.Assert.assertEquals;

public class NvHTTPTest {
    @Test
    public void appListStripsLegacyOrderZeroWidthPrefixFromAppTitle() throws Exception {
        LinkedList<NvApp> apps = NvHTTP.getAppListByReader(new StringReader(
                "<root status_code=\"200\">" +
                        "<App><ID>1</ID><AppTitle>&#x200b;&#x200c;MoonDeckStream</AppTitle></App>" +
                        "<App><ID>2</ID><AppTitle>Desktop</AppTitle></App>" +
                        "</root>"));
        assertEquals(2, apps.size());
        assertEquals("MoonDeckStream", apps.get(0).getAppName());
        assertEquals("Desktop", apps.get(1).getAppName());
        assertEquals("", NvHTTP.stripLegacyOrderPrefix(null));
        assertEquals("", NvHTTP.stripLegacyOrderPrefix("\u200B\u200C"));
    }
}

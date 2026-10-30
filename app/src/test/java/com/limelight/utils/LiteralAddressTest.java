package com.limelight.utils;

import org.junit.Test;

import java.net.Inet4Address;
import java.net.Inet6Address;
import java.net.InetAddress;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

public class LiteralAddressTest {
    @Test
    public void literalsAreParsed() {
        InetAddress lan = LiteralAddress.parse("192.168.4.12");
        assertTrue(lan instanceof Inet4Address);
        assertTrue(lan.isSiteLocalAddress());
        assertFalse(LiteralAddress.parse("8.8.8.8").isSiteLocalAddress());
        assertTrue(LiteralAddress.parse("fe80::1") instanceof Inet6Address);
        assertEquals(LiteralAddress.parse("fd00::1"), LiteralAddress.parse("[fd00::1]"));
        assertTrue(LiteralAddress.parse("::ffff:10.0.0.1").isSiteLocalAddress());
    }

    @Test
    public void namesAreNeverLookedUp() {
        // localhost resolves without a network; a lookup would answer 127.0.0.1.
        assertNull(LiteralAddress.parse("localhost"));
        assertNull(LiteralAddress.parse("gaming-pc.local"));
        assertNull(LiteralAddress.parse("example.com"));
    }

    @Test
    public void invalidTextIsNoAddress() {
        for (String text : new String[] {null, "", "[]", "[192.168.1.1]", "[fe80::1", "fe80::1]", "256.1.1.1",
                "1.2.3", "1.2.3.4.5", "1..2.3", "1.2.3.4 ", "fe80::1%wlan0", "fe80:::1:", "%3A%3A1", "0x7f.0.0.1"}) {
            assertNull(String.valueOf(text), LiteralAddress.parse(text));
        }
    }
}

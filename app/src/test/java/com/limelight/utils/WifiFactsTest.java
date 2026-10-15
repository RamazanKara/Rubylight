package com.limelight.utils;

import org.junit.Test;

import static com.limelight.utils.WifiFacts.UNKNOWN;
import static org.junit.Assert.*;

public class WifiFactsTest {
    private static WifiFacts wifi(int rssi, int frequency) {
        return new WifiFacts(WifiFacts.Link.WIFI, rssi, frequency, 80, 866, 780, 866, 5);
    }

    @Test public void bandsAndChannels() {
        assertEquals(WifiFacts.Band.GHZ_2_4, wifi(-50, 2412).band());
        assertEquals(1, wifi(-50, 2412).channel());
        assertEquals(14, wifi(-50, 2484).channel());
        assertEquals(WifiFacts.Band.GHZ_5, wifi(-50, 5180).band());
        assertEquals(36, wifi(-50, 5180).channel());
        assertEquals(165, wifi(-50, 5825).channel());
        assertEquals(WifiFacts.Band.GHZ_6, wifi(-50, 5955).band());
        assertEquals(1, wifi(-50, 5955).channel());
        assertEquals(2, wifi(-50, 5935).channel());
        assertEquals(233, wifi(-50, 7115).channel());
        assertNull(wifi(-50, 60480).band());
        assertEquals(UNKNOWN, wifi(-50, 60480).channel());
        assertNull(wifi(-50, -1).band());
    }

    @Test public void signalGradesAndWarnings() {
        assertEquals(WifiFacts.Signal.EXCELLENT, wifi(-50, 5180).signal());
        assertEquals(WifiFacts.Signal.GOOD, wifi(-67, 5180).signal());
        assertEquals(WifiFacts.Signal.FAIR, wifi(-75, 5180).signal());
        assertEquals(WifiFacts.Signal.WEAK, wifi(-76, 5180).signal());
        assertTrue(wifi(-80, 2437).slowBand());
        assertTrue(wifi(-80, 2437).weakSignal());
        assertFalse(wifi(-50, 5180).slowBand());
        assertFalse(wifi(-50, 5180).weakSignal());
    }

    @Test public void placeholderValuesStayUnknown() {
        // Android reports -127 dBm and -1 Mbps when it has no value.
        WifiFacts facts = new WifiFacts(WifiFacts.Link.WIFI, -127, 0, 0, -1, -1, -1, UNKNOWN);
        assertEquals(UNKNOWN, facts.rssiDbm);
        assertNull(facts.signal());
        assertEquals(UNKNOWN, facts.frequencyMhz);
        assertEquals(UNKNOWN, facts.txMbps);
        assertFalse(facts.slowBand());
        assertFalse(facts.weakSignal());
    }

    @Test public void otherLinksCarryNoWifiWarnings() {
        WifiFacts ethernet = WifiFacts.notWifi(WifiFacts.Link.ETHERNET);
        assertFalse(ethernet.onWifi());
        assertFalse(ethernet.slowBand());
        assertNull(ethernet.band());
        assertNull(WifiFacts.notWifi(null).link);
    }

    @Test public void standardsMapToGenerations() {
        assertEquals(0, WifiFacts.generationOf(1));
        assertEquals(4, WifiFacts.generationOf(4));
        assertEquals(5, WifiFacts.generationOf(5));
        assertEquals(6, WifiFacts.generationOf(6));
        assertEquals(7, WifiFacts.generationOf(8));
        assertEquals(UNKNOWN, WifiFacts.generationOf(0));
        assertEquals(UNKNOWN, WifiFacts.generationOf(7));
    }

    private static byte[] bytes(int... values) {
        byte[] out = new byte[values.length];
        for (int i = 0; i < values.length; i++) out[i] = (byte) values[i];
        return out;
    }

    @Test public void htOperationGives20Or40() {
        assertEquals(20, WifiFacts.widthFromElement(61, 0, bytes(6, 0x00, 0, 0, 0)));
        assertEquals(40, WifiFacts.widthFromElement(61, 0, bytes(36, 0x05, 0, 0, 0)));
        assertEquals(UNKNOWN, WifiFacts.widthFromElement(61, 0, bytes(36)));
    }

    @Test public void vhtOperationGives80Or160() {
        assertEquals(UNKNOWN, WifiFacts.widthFromElement(191, 0, bytes(0, 0, 0, 0, 0)));
        assertEquals(80, WifiFacts.widthFromElement(191, 0, bytes(1, 42, 0, 0, 0)));
        assertEquals(160, WifiFacts.widthFromElement(191, 0, bytes(1, 42, 50, 0, 0)));
        assertEquals(160, WifiFacts.widthFromElement(191, 0, bytes(2, 50, 0, 0, 0)));
        assertEquals(UNKNOWN, WifiFacts.widthFromElement(191, 0, bytes(1, 42)));
    }

    @Test public void heOperationReadsOptionalVhtAnd6GhzFields() {
        // No optional fields.
        assertEquals(UNKNOWN, WifiFacts.widthFromElement(255, 36, bytes(0, 0, 0, 0, 0, 0)));
        // VHT information present: 80 MHz.
        assertEquals(80, WifiFacts.widthFromElement(255, 36, bytes(0, 0x40, 0, 0, 0, 0, 1, 42, 0)));
        // Co-hosted BSSID indicator then 6 GHz information: control width 3 means 160 MHz.
        assertEquals(160, WifiFacts.widthFromElement(255, 36,
                bytes(0, 0x80, 0x02, 0, 0, 0, 1, 37, 0x03, 39, 0, 0)));
        assertEquals(80, WifiFacts.widthFromElement(255, 36, bytes(0, 0, 0x02, 0, 0, 0, 37, 0x02, 39, 0, 0)));
        // Truncated optional fields do not read past the body.
        assertEquals(UNKNOWN, WifiFacts.widthFromElement(255, 36, bytes(0, 0x40, 0, 0, 0, 0, 1)));
    }

    @Test public void ehtOperationReads320() {
        assertEquals(320, WifiFacts.widthFromElement(255, 106, bytes(0x01, 0, 0, 0, 0, 0x04, 31, 63)));
        assertEquals(UNKNOWN, WifiFacts.widthFromElement(255, 106, bytes(0x00, 0, 0, 0, 0)));
        assertEquals(UNKNOWN, WifiFacts.widthFromElement(255, 106, bytes(0x01, 0, 0, 0, 0)));
    }

    @Test public void unrelatedElementsSayNothing() {
        assertEquals(UNKNOWN, WifiFacts.widthFromElement(0, 0, bytes('n', 'e', 't')));
        assertEquals(UNKNOWN, WifiFacts.widthFromElement(255, 35, bytes(1, 2, 3, 4, 5, 6)));
        assertEquals(UNKNOWN, WifiFacts.widthFromElement(61, 0, null));
    }
}

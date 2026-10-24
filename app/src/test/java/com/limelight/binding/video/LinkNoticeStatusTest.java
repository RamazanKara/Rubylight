package com.limelight.binding.video;

import org.junit.Test;

import java.util.Locale;

import static org.junit.Assert.*;

public class LinkNoticeStatusTest {
    @Test
    public void onlyALoweredBitrateIsAStatus() {
        LinkNoticeStatus limited = LinkNoticeStatus.parse(1, 150_000, 75_000);
        assertNotNull(limited);
        assertEquals(LinkNoticeStatus.STATE_LIMITED, limited.state);
        assertEquals(150_000, limited.ceilingKbps);
        assertEquals(75_000, limited.currentKbps);
        assertEquals(LinkNoticeStatus.STATE_FLOOR, LinkNoticeStatus.parse(2, 150_000, 4_000).state);
        // The host back at full rate clears the notice, and nonsense never makes one.
        assertNull(LinkNoticeStatus.parse(0, 150_000, 150_000));
        assertNull(LinkNoticeStatus.parse(3, 150_000, 75_000));
        assertNull(LinkNoticeStatus.parse(-1, 150_000, 75_000));
        assertNull(LinkNoticeStatus.parse(1, 0, 75_000));
        assertNull(LinkNoticeStatus.parse(1, 150_000, 0));
    }

    @Test
    public void mbpsAreWholeFromTenAndOneDecimalBelow() {
        assertEquals("75", LinkNoticeStatus.parse(1, 150_000, 75_000).currentMbps(Locale.US));
        assertEquals("10", LinkNoticeStatus.parse(1, 150_000, 10_000).currentMbps(Locale.US));
        assertEquals("9.9", LinkNoticeStatus.parse(1, 150_000, 9_900).currentMbps(Locale.US));
        assertEquals("4,5", LinkNoticeStatus.parse(2, 150_000, 4_500).currentMbps(Locale.GERMANY));
        assertEquals("0.5", LinkNoticeStatus.parse(2, 150_000, 500).currentMbps(Locale.US));
    }
}

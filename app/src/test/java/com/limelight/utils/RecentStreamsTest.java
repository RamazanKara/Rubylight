package com.limelight.utils;

import org.junit.Before;
import org.junit.Test;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

public class RecentStreamsTest {
    @Before
    public void clear() {
        RecentStreams.clear();
    }

    @Test public void noStreamsSaysSo() {
        assertEquals("Recent streams, oldest first (up to 8, times in UTC):\nNone finished in this app process.\n\n",
                RecentStreams.section());
    }

    @Test public void theOldestRowGoesFirstWhenFull() {
        for (int i = 1; i <= RecentStreams.MAX_ROWS + 1; i++) {
            RecentStreams.add("stream " + i);
        }
        String section = RecentStreams.section();
        assertFalse(section, section.contains("stream 1 "));
        assertTrue(section, section.contains("stream 2 |"));
        assertTrue(section, section.contains("stream 9 |"));
        assertTrue(section, section.indexOf("stream 2 ") < section.indexOf("stream 9 "));
    }

    @Test public void aRowCarriesTheLinkNotedForItsStreamOnly() {
        RecentStreams.noteLink(new WiredLink.Hints(1000, false));
        RecentStreams.add("wired");
        RecentStreams.noteLink(new WiredLink.Hints(0, true));
        RecentStreams.add("wireless");
        RecentStreams.add("not noted");
        String section = RecentStreams.section();
        assertTrue(section, section.contains("wired | link wired 1000 Mbps\n"));
        assertTrue(section, section.contains("wireless | link wireless\n"));
        assertTrue(section, section.contains("not noted | link unknown\n"));
    }
}

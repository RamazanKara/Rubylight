package com.limelight.binding.video;

import org.junit.Test;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;

public class InputStallLineTest {
    private static final long START = 500_000;

    @Test public void theFirstWaitGetsALineAtOnce() {
        assertEquals("Decoder input waits over 20 ms: 1, worst 25 ms", new InputStallLine().onStall(START, 25));
    }

    @Test public void waitsWithinTenSecondsCoalesceIntoTheNextLineWithCountAndWorst() {
        InputStallLine stalls = new InputStallLine();
        stalls.onStall(START, 25);
        assertNull(stalls.onStall(START + 100, 40));
        assertNull(stalls.onStall(START + 9_999, 21));
        assertEquals("Decoder input waits over 20 ms: 3, worst 40 ms", stalls.onStall(START + 10_000, 22));
        // The next window starts over.
        assertNull(stalls.onStall(START + 10_100, 30));
    }

    @Test public void aHundredWaitsMakeNoMoreThanOneLinePerTenSeconds() {
        InputStallLine stalls = new InputStallLine();
        int lines = 0;
        for (int i = 0; i < 100; i++) {
            if (stalls.onStall(START + i * 1_000L, 25) != null) lines++;
        }
        assertEquals(10, lines);
        assertEquals(100, stalls.total());
    }

    @Test public void theEndOfAStreamKeepsItsLastWaits() {
        InputStallLine stalls = new InputStallLine();
        assertNull(stalls.flush());
        stalls.onStall(START, 25);
        assertNull("that one was reported", stalls.flush());
        stalls.onStall(START + 100, 33);
        assertEquals("Decoder input waits over 20 ms: 1, worst 33 ms", stalls.flush());
        assertNull(stalls.flush());
        assertEquals(33, stalls.totalMaxMs());
    }
}

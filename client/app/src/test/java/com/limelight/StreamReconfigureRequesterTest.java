package com.limelight;

import org.junit.Test;
import static org.junit.Assert.*;

public class StreamReconfigureRequesterTest {
    private static StreamReconfigureRequester foldOpen() {
        StreamReconfigureRequester requester = new StreamReconfigureRequester();
        requester.start(2176, 1812, 120_000, 2176, 1812, 120_000);
        return requester;
    }

    @Test
    public void waitsUntilChangesSettle() {
        StreamReconfigureRequester requester = foldOpen();
        assertEquals(StreamReconfigureRequester.SETTLE_MS, requester.onChange(1000));
        assertFalse(requester.settled(1299));
        // Another change restarts the wait.
        requester.onChange(1200);
        assertFalse(requester.settled(1300));
        assertEquals(200, requester.remainingMs(1300));
        assertTrue(requester.settled(1500));
        assertEquals(0, requester.remainingMs(1600));
    }

    @Test
    public void aLongerWaitIsNotCutShortByLaterChanges() {
        StreamReconfigureRequester requester = foldOpen();
        // A turn waits a second; the window and display changes it brings wait no less.
        assertEquals(1000, requester.onChange(1000, 1000));
        assertEquals(900, requester.onChange(1100));
        assertFalse(requester.settled(1999));
        assertTrue(requester.settled(2000));
        // Once settled, the next change waits its own time again.
        assertEquals(StreamReconfigureRequester.SETTLE_MS, requester.onChange(3000));
    }

    @Test
    public void turningThePhoneAsksForEachShape() {
        StreamReconfigureRequester requester = new StreamReconfigureRequester();
        requester.start(2400, 1080, 60_000, 2400, 1080, 60_000);
        assertNull(requester.decide(2400, 1080, 60_000));
        int[] portrait = requester.decide(1080, 2400, 60_000);
        assertArrayEquals(new int[] {1080, 2400, 60_000}, portrait);
        requester.sent(portrait);
        assertArrayEquals(new int[] {2400, 1080, 60_000}, requester.decide(2400, 1080, 60_000));
    }

    @Test
    public void closingAndOpeningTheFoldRequestsEachPanel() {
        StreamReconfigureRequester requester = foldOpen();
        int[] cover = requester.decide(2316, 904, 120_000);
        assertArrayEquals(new int[] {2316, 904, 120_000}, cover);
        requester.sent(cover);
        assertNull(requester.decide(2316, 904, 120_000));
        int[] inner = requester.decide(2176, 1812, 120_000);
        assertArrayEquals(new int[] {2176, 1812, 120_000}, inner);
        requester.sent(inner);
        assertEquals(2176, requester.width());
        assertEquals(1812, requester.height());
    }

    @Test
    public void unchangedOfferSendsNothing() {
        StreamReconfigureRequester requester = foldOpen();
        assertNull(requester.decide(2176, 1812, 120_000));
    }

    @Test
    public void streamThatNeverMatchedTheOfferIsLeftAloneUntilTheOfferMoves() {
        // Started in split screen at the full panel size: rotating or redrawing the same window
        // keeps the size the person started with.
        StreamReconfigureRequester requester = new StreamReconfigureRequester();
        requester.start(2176, 1812, 120_000, 1086, 1812, 120_000);
        assertNull(requester.decide(1086, 1812, 120_000));
        assertArrayEquals(new int[] {1450, 1812, 120_000}, requester.decide(1450, 1812, 120_000));
    }

    @Test
    public void offerReturningToTheStreamSizeResetsTheBaseline() {
        StreamReconfigureRequester requester = foldOpen();
        // The window shrank and grew back before anything was sent.
        assertNull(requester.decide(2176, 1812, 120_000));
        assertArrayEquals(new int[] {1086, 1812, 120_000}, requester.decide(1086, 1812, 120_000));
        assertNull(requester.decide(2176, 1812, 120_000));
        assertNull(requester.decide(2176, 1812, 120_000));
    }

    @Test
    public void unsentRequestIsTriedAgain() {
        StreamReconfigureRequester requester = foldOpen();
        assertNotNull(requester.decide(2316, 904, 120_000));
        // Not sent (control stream busy or decoder could not take it): the same offer asks again.
        assertArrayEquals(new int[] {2316, 904, 120_000}, requester.decide(2316, 904, 120_000));
        assertEquals(2176, requester.width());
    }

    @Test
    public void frameRateAloneCanChange() {
        StreamReconfigureRequester requester = foldOpen();
        int[] request = requester.decide(2176, 1812, 60_000);
        assertArrayEquals(new int[] {2176, 1812, 60_000}, request);
        requester.sent(request);
        assertEquals(60_000, requester.fpsMillihz());
    }

    @Test
    public void nothingIsDecidedBeforeTheStreamStarts() {
        assertNull(new StreamReconfigureRequester().decide(2316, 904, 120_000));
    }
}

package com.limelight.utils;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertTrue;

import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.StreamMove;
import com.limelight.nativeapi.StreamMoveFixtures;
import com.limelight.nvstream.http.NvApp;

import org.junit.Test;

/** Which starts ask "X is playing on Y". */
public class ContinueHereGateTest {
    private static NvApp game(int id) {
        return new NvApp("Game", id, false);
    }

    @Test
    public void aPlainStreamOfTheGameAnotherDevicePlaysAsks() {
        assertTrue(ContinueHereGate.plainStart(game(7), false, LaunchSource.LIBRARY_TAP));
        assertTrue(ContinueHereGate.asks(true, true, StreamMoveFixtures.playing("Phone", 7), game(7)));
    }

    @Test
    public void theReadGivesUpAtItsDeadlineOrOnAFailureAndTheStartGoesOn() {
        StreamMove move = StreamMoveFixtures.playing("Phone", 7);
        assertSame(move, ContinueHereGate.readWithin(() -> move, 5_000));
        assertNull(ContinueHereGate.readWithin(() -> {
            throw new NativeApiException(NativeApiException.Kind.NETWORK, "refused");
        }, 5_000));
        long started = System.nanoTime();
        assertNull(ContinueHereGate.readWithin(() -> {
            Thread.sleep(5_000);
            return move;
        }, 20));
        assertTrue("it does not wait for the read", System.nanoTime() - started < 4_000_000_000L);
    }

    @Test
    public void anythingElseStartsAsItAlwaysDid() {
        assertFalse(ContinueHereGate.plainStart(game(7), true, LaunchSource.LIBRARY_TAP));
        assertFalse(ContinueHereGate.plainStart(game(7), false, LaunchSource.COOP_JOIN));
        assertFalse(ContinueHereGate.plainStart(game(7), false, LaunchSource.RECOVERY));
        // Another game, nobody else, a host without the feature.
        assertFalse(ContinueHereGate.asks(true, true, StreamMoveFixtures.playing("Phone", 8), game(7)));
        assertFalse(ContinueHereGate.asks(true, true, StreamMoveFixtures.playing(null, 7), game(7)));
        assertFalse(ContinueHereGate.asks(false, true, StreamMoveFixtures.playing("Phone", 7), game(7)));
    }
}

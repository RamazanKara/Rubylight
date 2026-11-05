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

    // The host lists the game under its own name first for a device beside it, with another id and UUID.
    @Test
    public void theTilesThatReachTheRunningGameAskToo() {
        for (String uuid : new String[]{"9a1c5a25-58fe-40e0-b9aa-7d3f00000007", "9a1c5a25-58fe-40e0-b9aa-7d3f00000001"}) {
            NvApp tile = game(1941625188);
            tile.setAppUuid(uuid);
            assertTrue(uuid, ContinueHereGate.plainStart(tile, false, LaunchSource.LIBRARY_TAP));
            assertTrue(uuid, ContinueHereGate.asks(true, true, StreamMoveFixtures.playing("Phone", 7), tile));
            assertFalse(uuid, ContinueHereGate.asks(true, true, StreamMoveFixtures.playing(null, 7), tile));
        }
        NvApp terminate = game(2147483504);
        assertFalse(ContinueHereGate.plainStart(terminate, false, LaunchSource.LIBRARY_TAP));
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
        assertFalse("its page finishes at once, which would drop the start",
                ContinueHereGate.plainStart(game(7), false, LaunchSource.OPEN_PLAY));
        // Another game, nobody else, a host without the feature.
        assertFalse(ContinueHereGate.asks(true, true, StreamMoveFixtures.playing("Phone", 8), game(7)));
        assertFalse(ContinueHereGate.asks(true, true, StreamMoveFixtures.playing(null, 7), game(7)));
        assertFalse(ContinueHereGate.asks(false, true, StreamMoveFixtures.playing("Phone", 7), game(7)));
    }
}

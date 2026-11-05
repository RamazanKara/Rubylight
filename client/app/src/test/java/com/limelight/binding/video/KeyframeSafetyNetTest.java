package com.limelight.binding.video;

import org.junit.Test;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

public class KeyframeSafetyNetTest {
    private static final long START = 500_000;

    private static KeyframeSafetyNet started() {
        KeyframeSafetyNet net = new KeyframeSafetyNet();
        net.onKeyframe(START);
        return net;
    }

    @Test public void nothingIsAskedBeforeTheFirstKeyframe() {
        KeyframeSafetyNet net = new KeyframeSafetyNet();
        assertFalse(net.due(START));
        assertFalse(net.due(START + 60_000));
        assertEquals(0, net.requests());
    }

    @Test public void itIsDueTenSecondsAfterAKeyframeAndNotBefore() {
        KeyframeSafetyNet net = started();
        assertFalse(net.due(START + 9_999));
        assertTrue(net.due(START + 10_000));
        assertEquals(1, net.requests());
    }

    @Test public void anyKeyframeRestartsTheWait() {
        KeyframeSafetyNet net = started();
        net.onKeyframe(START + 9_000);
        assertFalse(net.due(START + 10_000));
        assertFalse(net.due(START + 18_999));
        assertTrue(net.due(START + 19_000));
    }

    @Test public void anUnansweredRequestWaitsAnotherTenSeconds() {
        KeyframeSafetyNet net = started();
        assertTrue(net.due(START + 10_000));
        // The frames still in flight must not ask again.
        assertFalse(net.due(START + 10_016));
        assertFalse(net.due(START + 19_999));
        assertTrue(net.due(START + 20_000));
        assertEquals(2, net.requests());
    }

    @Test public void theAnswerRestartsTheWaitFromItsArrival() {
        KeyframeSafetyNet net = started();
        assertTrue(net.due(START + 10_000));
        net.onKeyframe(START + 10_040);
        assertFalse(net.due(START + 20_000));
        assertTrue(net.due(START + 20_040));
    }
}

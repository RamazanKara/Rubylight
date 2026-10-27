package com.limelight.binding.input.touch;

import org.junit.Test;

import static org.junit.Assert.*;

public class TrackpadAreaTest {
    @Test
    public void fullScreenAcceptsEverything() {
        TrackpadArea area = TrackpadArea.forPreset(TrackpadArea.FULL);
        assertTrue(area.isFull());
        assertTrue(area.contains(0, 0, 1000, 500));
        assertTrue(area.contains(-5, 900, 1000, 500));
        assertTrue(TrackpadArea.forPreset(null).isFull());
        assertTrue(TrackpadArea.forPreset("unknown").isFull());
    }

    @Test
    public void halvesSplitTheScreen() {
        TrackpadArea right = TrackpadArea.forPreset(TrackpadArea.RIGHT_HALF);
        assertFalse(right.isFull());
        assertTrue(right.contains(600, 10, 1000, 500));
        assertTrue(right.contains(500, 250, 1000, 500));
        assertFalse(right.contains(499, 250, 1000, 500));

        TrackpadArea left = TrackpadArea.forPreset(TrackpadArea.LEFT_HALF);
        assertTrue(left.contains(0, 499, 1000, 500));
        assertFalse(left.contains(501, 250, 1000, 500));

        TrackpadArea bottom = TrackpadArea.forPreset(TrackpadArea.BOTTOM_HALF);
        assertTrue(bottom.contains(10, 400, 1000, 500));
        assertFalse(bottom.contains(10, 100, 1000, 500));

        TrackpadArea top = TrackpadArea.forPreset(TrackpadArea.TOP_HALF);
        assertTrue(top.contains(10, 100, 1000, 500));
        assertFalse(top.contains(10, 400, 1000, 500));
    }

    @Test
    public void touchesOffTheScreenAreOutside() {
        TrackpadArea right = TrackpadArea.forPreset(TrackpadArea.RIGHT_HALF);
        assertFalse(right.contains(1001, 250, 1000, 500));
        assertFalse(right.contains(700, -1, 1000, 500));
    }

    @Test
    public void anUnmeasuredScreenDoesNotBlockInput() {
        TrackpadArea right = TrackpadArea.forPreset(TrackpadArea.RIGHT_HALF);
        assertTrue(right.contains(0, 0, 0, 0));
        assertTrue(right.contains(0, 0, Float.NaN, 500));
    }
}

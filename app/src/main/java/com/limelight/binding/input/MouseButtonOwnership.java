package com.limelight.binding.input;

/** Combines ordinary mouse input with held overlay buttons without premature releases. */
public final class MouseButtonOwnership {
    private final boolean[] ordinary = new boolean[6], overlay = new boolean[6];
    /** 1 = press, -1 = release, 0 = no host transition. Caller serializes update and sending. */
    public int update(int button, boolean down, boolean fromOverlay) {
        if (button < 1 || button >= ordinary.length) return 0;
        boolean previous = ordinary[button] || overlay[button];
        (fromOverlay ? overlay : ordinary)[button] = down;
        boolean next = ordinary[button] || overlay[button];
        return previous == next ? 0 : next ? 1 : -1;
    }
}

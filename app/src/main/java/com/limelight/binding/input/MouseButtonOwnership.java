package com.limelight.binding.input;

/** Combines ordinary mouse input with held overlay buttons without premature releases. */
public final class MouseButtonOwnership {
    public static final int ORDINARY = 0, GAME_CONTROLS = 1, DESKTOP_CONTROLS = 2;
    private final int[] owners = new int[6];
    /** 1 = press, -1 = release, 0 = no host transition. Caller serializes update and sending. */
    public int update(int button, boolean down, boolean fromOverlay) {
        return update(button, down, fromOverlay ? GAME_CONTROLS : ORDINARY);
    }

    /** Each input surface owns its hold independently, including duplicate down events. */
    public int update(int button, boolean down, int source) {
        if (button < 1 || button >= owners.length || source < ORDINARY || source > DESKTOP_CONTROLS) return 0;
        boolean previous = owners[button] != 0;
        int bit = 1 << source;
        if (down) owners[button] |= bit;
        else owners[button] &= ~bit;
        boolean next = owners[button] != 0;
        return previous == next ? 0 : next ? 1 : -1;
    }
}

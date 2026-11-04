package com.limelight.binding.input.pad;

import com.limelight.binding.input.LayoutStick;

/**
 * The keys the modern controller's WASD stick holds for a stick position. Eight equal sectors: W, A, S and D, and
 * between them the diagonals, which hold both neighbours. A held sector is kept until the stick leaves it by
 * {@link #HYSTERESIS} degrees, and a held key until the stick falls back near the deadzone, so a thumb resting on a
 * border never flickers a key. Plain logic, no allocation.
 */
final class WasdKeys {
    /** Windows virtual-key codes, as the PC keyboard sends them. */
    static final int VK_W = 0x57, VK_A = 0x41, VK_S = 0x53, VK_D = 0x44;
    /** Degrees past a sector's border before the next sector takes over. */
    static final float HYSTERESIS = 8;
    /** Past the deadzone, how far (of full deflection) the stick moves before a key goes down, and where it lets go. */
    static final float ENGAGE = .12f, LET_GO = .05f;
    /** No sector: nothing held. Sectors count counter-clockwise from D, so even ones hold one key, odd ones two. */
    static final int NONE = -1;
    /** The keys of each sector: D, W+D, W, W+A, A, S+A, S, S+D. */
    private static final int[][] SECTOR_KEYS = {
            {VK_D}, {VK_W, VK_D}, {VK_W}, {VK_W, VK_A}, {VK_A}, {VK_S, VK_A}, {VK_S}, {VK_S, VK_D}};
    private static final int[] NO_KEYS = {};

    /** What a stick move should feel like: nothing, one key entered, or a diagonal entered. */
    enum Feedback { NONE, DIRECTION, DIAGONAL }

    interface Keys {
        void key(int vk, boolean down);
    }

    private final Keys keys;
    private int sector = NONE;

    WasdKeys(Keys keys) {
        this.keys = keys;
    }

    /** The sector held, from 0 (D) counter-clockwise to 7 (S+D), or {@link #NONE}. */
    int sector() {
        return sector;
    }

    /** The held keys as {@link LayoutStick} direction bits (W up, A left, S down, D right), for drawing. */
    int directions() {
        return (holds(VK_W) ? LayoutStick.UP : 0) | (holds(VK_A) ? LayoutStick.LEFT : 0)
                | (holds(VK_S) ? LayoutStick.DOWN : 0) | (holds(VK_D) ? LayoutStick.RIGHT : 0);
    }

    /** Whether {@code vk} is held now. */
    boolean holds(int vk) {
        return sector != NONE && contains(SECTOR_KEYS[sector], vk);
    }

    /**
     * Moves to the stick position ({@code x}, {@code y}), -1 to 1 with up positive and the deadzone already taken
     * out (0 inside it), sending the keys that change. Returns the feedback for entering a new sector; leaving every
     * sector, staying in one or moving inside the deadzone gives {@link Feedback#NONE}.
     */
    Feedback update(float x, float y) {
        int next = sectorFor(x, y, sector);
        if (next == sector) return Feedback.NONE;
        int[] before = sector == NONE ? NO_KEYS : SECTOR_KEYS[sector];
        int[] after = next == NONE ? NO_KEYS : SECTOR_KEYS[next];
        sector = next;
        // Let go of what the new sector drops before pressing what it adds, so W to D never holds both.
        for (int vk : before) if (!contains(after, vk)) keys.key(vk, false);
        for (int vk : after) if (!contains(before, vk)) keys.key(vk, true);
        if (next == NONE) return Feedback.NONE;
        return next % 2 == 0 ? Feedback.DIRECTION : Feedback.DIAGONAL;
    }

    /** Lets go of every held key: lift, cancel, a hidden or removed stick, the stream's end or lost focus. */
    void release() {
        update(0, 0);
    }

    /** The sector for ({@code x}, {@code y}) when {@code current} is held now. */
    static int sectorFor(float x, float y, int current) {
        float magnitude = (float) Math.hypot(x, y);
        if (!(magnitude >= (current == NONE ? ENGAGE : LET_GO))) return NONE;
        double angle = Math.toDegrees(Math.atan2(y, x));
        if (current != NONE && Math.abs(difference(angle, current * 45)) <= 22.5 + HYSTERESIS) return current;
        return (int) Math.round((angle < 0 ? angle + 360 : angle) / 45) % 8;
    }

    private static double difference(double angle, double target) {
        double value = (angle - target) % 360;
        if (value > 180) value -= 360;
        if (value < -180) value += 360;
        return value;
    }

    private static boolean contains(int[] values, int value) {
        for (int v : values) if (v == value) return true;
        return false;
    }
}

package com.limelight.binding.input;

import com.limelight.nvstream.input.ControllerPacket;

/** Turns one finger on a stick, D-pad or WASD control into a stick position. Plain math, no allocation. */
public final class LayoutStick {
    public static final int UP = 1, LEFT = 2, DOWN = 4, RIGHT = 8;
    /** A D-pad ignores a finger this close to its center and splits the rest into eight equal sectors. */
    public static final float DPAD_DEADZONE = .25f, DPAD_DIAGONAL = 45;

    private final boolean floating;
    private final float deadzone;
    private float centerX, centerY, radius;
    private boolean active;
    /** Position from -1 to 1, with up positive. */
    public float x, y;

    public LayoutStick(boolean floating, float deadzone) {
        this.floating = floating;
        this.deadzone = deadzone;
    }

    /**
     * A finger landed at ({@code touchX}, {@code touchY}) in a control of the given size. A floating
     * stick centers there; a fixed one keeps the control's center and moves at once.
     */
    public void down(float touchX, float touchY, float width, float height, float radius) {
        this.radius = Math.max(1, radius);
        active = true;
        if (floating) {
            centerX = touchX; centerY = touchY;
        } else {
            centerX = width / 2; centerY = height / 2;
        }
        move(touchX, touchY);
    }

    public void move(float touchX, float touchY) {
        if (!active) return;
        float dx = (touchX - centerX) / radius, dy = (centerY - touchY) / radius;
        float length = (float) Math.sqrt(dx * dx + dy * dy);
        if (length <= deadzone || length == 0) {
            x = 0; y = 0;
            return;
        }
        // Rescale past the deadzone so the stick still reaches every value from just above 0 to 1.
        float scaled = (Math.min(length, 1) - deadzone) / (1 - deadzone);
        x = dx / length * scaled;
        y = dy / length * scaled;
    }

    public void release() {
        active = false;
        x = 0; y = 0;
    }

    public boolean active() { return active; }
    public float centerX() { return centerX; }
    public float centerY() { return centerY; }

    /**
     * The held directions for a stick position, as {@link #UP}, {@link #LEFT}, {@link #DOWN} and
     * {@link #RIGHT} bits. Each direction covers 90 degrees plus {@code diagonal}, so 0 never holds two
     * keys, 45 gives eight equal sectors and 90 holds two keys everywhere off the axes.
     */
    public static int directions(float x, float y, float diagonal) {
        if (x == 0 && y == 0) return 0;
        double angle = Math.toDegrees(Math.atan2(y, x));
        double half = 45 + diagonal / 2;
        int result = 0;
        if (Math.abs(difference(angle, 90)) < half) result |= UP;
        if (Math.abs(difference(angle, 180)) < half) result |= LEFT;
        if (Math.abs(difference(angle, -90)) < half) result |= DOWN;
        if (Math.abs(difference(angle, 0)) < half) result |= RIGHT;
        if (result == 0) {
            // Exactly on a boundary with no overlap: take the direction the angle rounds to.
            int nearest = (int) Math.round(angle / 90) & 3;
            result = nearest == 0 ? RIGHT : nearest == 1 ? UP : nearest == 2 ? LEFT : DOWN;
        }
        return result;
    }

    private static double difference(double angle, double target) {
        double value = (angle - target) % 360;
        if (value > 180) value -= 360;
        if (value < -180) value += 360;
        return value;
    }

    /** Gamepad D-pad flags for the given {@link #directions} bits. */
    public static int dpadFlags(int directions) {
        int flags = 0;
        if ((directions & UP) != 0) flags |= ControllerPacket.UP_FLAG;
        if ((directions & LEFT) != 0) flags |= ControllerPacket.LEFT_FLAG;
        if ((directions & DOWN) != 0) flags |= ControllerPacket.DOWN_FLAG;
        if ((directions & RIGHT) != 0) flags |= ControllerPacket.RIGHT_FLAG;
        return flags;
    }

    /** Moves a control's edge onto the nearest grid line, keeping the control on the surface. */
    public static float snap(float position, float size, float surface, float grid) {
        float max = Math.max(0, surface - size);
        float value = grid > 0 ? Math.round(position / grid) * grid : position;
        // A control that cannot reach the far edge on the grid can still sit flush against it.
        if (grid > 0 && Math.abs(max - position) < Math.abs(value - position)) value = max;
        return Math.max(0, Math.min(max, value));
    }
}

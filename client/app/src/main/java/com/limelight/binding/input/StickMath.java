package com.limelight.binding.input;

/**
 * The on-screen stick's geometry and mapping. The spec is the shared crate (nightjar-policy's
 * {@code stick} module); this is its Java port, the same float operations in the same order, and
 * PolicyTraceTest replays the crate's traces in protocol/policy/tests/traces/stick against it, so
 * both stay bit-equal. Plain Java on purpose: the input and draw paths make no JNI calls.
 *
 * <p>Sizes are pixels of a square control; deflections run from -1 to 1 with up positive. The pair
 * forms write x into {@code out[0]} and y into {@code out[1]} and allocate nothing.
 */
public final class StickMath {
    private StickMath() {}

    private static final float KNOB = .2f, TRAVEL = .3f;

    /** How far the finger moves from the stick's centre for full deflection. */
    public static float travel(float size) {
        return size * TRAVEL;
    }

    public static float knobRadius(float size) {
        return size * KNOB;
    }

    /** The base's radius: half the side, less the rim's stroke. */
    public static float baseRadius(float size, float stroke) {
        return size / 2 - stroke;
    }

    /**
     * How far past each edge of the control the knob and its shadow ({@code shadow} past the knob's
     * edge) can reach: the knob's centre travels to the base's rim, so half the knob leaves the base.
     */
    public static float overhang(float size, float stroke, float shadow) {
        return Math.max(0, baseRadius(size, stroke) + knobRadius(size) + shadow - size / 2);
    }

    /**
     * The deflection for a finger {@code dx} right and {@code dy} up of the centre, full at
     * {@code radius}; 0 inside {@code deadzone} (a share of the radius), rescaled past it, and held
     * at full past the radius.
     */
    public static void deflection(float dx, float dy, float radius, float deadzone, float[] out) {
        radius = Math.max(1, radius);
        dx = dx / radius;
        dy = dy / radius;
        float length = (float) Math.sqrt(dx * dx + dy * dy);
        if (length <= deadzone || length == 0) {
            out[0] = 0;
            out[1] = 0;
            return;
        }
        float scaled = (Math.min(length, 1) - deadzone) / (1 - deadzone);
        out[0] = dx / length * scaled;
        out[1] = dy / length * scaled;
    }

    /**
     * The knob's drawn centre in screen pixels: from the origin (the base's centre, or where a
     * floating stick's thumb landed) by the deflection times the base's radius {@code base}, never
     * past the rim around the centre. At full deflection of a centred stick it sits on the rim.
     */
    public static void knob(float centerX, float centerY, float originX, float originY, float x, float y,
                            float base, float[] out) {
        float kx = originX + x * base, ky = originY - y * base;
        float ox = kx - centerX, oy = ky - centerY;
        float off = (float) Math.sqrt(ox * ox + oy * oy);
        if (off > base && off > 0) {
            out[0] = centerX + ox / off * base;
            out[1] = centerY + oy / off * base;
        } else {
            out[0] = kx;
            out[1] = ky;
        }
    }

    // Single values, as the crate's traces call them (deflection_x and so on); not for the hot paths.

    public static float deflectionX(float dx, float dy, float radius, float deadzone) {
        float[] out = new float[2];
        deflection(dx, dy, radius, deadzone, out);
        return out[0];
    }

    public static float deflectionY(float dx, float dy, float radius, float deadzone) {
        float[] out = new float[2];
        deflection(dx, dy, radius, deadzone, out);
        return out[1];
    }

    public static float knobX(float centerX, float centerY, float originX, float originY, float x, float y, float base) {
        float[] out = new float[2];
        knob(centerX, centerY, originX, originY, x, y, base, out);
        return out[0];
    }

    public static float knobY(float centerX, float centerY, float originX, float originY, float x, float y, float base) {
        float[] out = new float[2];
        knob(centerX, centerY, originX, originY, x, y, base, out);
        return out[1];
    }
}

package com.limelight.binding.input.keyboard;

/**
 * Everything behind the on-screen keyboard that is not drawing: touches and controller buttons go in,
 * key events come out through the engine. A gamepad, a TV remote and the touch screen all work at once.
 *
 * <p>Controller mapping: the D-pad or left stick moves the selection, A presses the selected key
 * (hold to repeat), B closes the keyboard, X is Backspace, Y is Space, Start is Enter, and LB and RB
 * arm, lock and release Shift and Ctrl like tapping those keys. A remote uses the D-pad and OK.
 */
public final class PcKeyboardController {
    public enum Pad { UP, DOWN, LEFT, RIGHT, A, B, X, Y, LB, RB, START }

    public interface Host {
        void close();
        /** What is drawn changed: a selection, a modifier or a pressed key. */
        void changed();
    }

    /** First move after a D-pad direction or stick push, then the pace while it is held. */
    public static final long NAVIGATE_DELAY_MS = 350;
    public static final long NAVIGATE_REPEAT_MS = 100;
    private static final float STICK_ENGAGE = 0.6f, STICK_RELEASE = 0.35f;

    private final Host host;
    private final PcKeyboardEngine engine;
    private PcKeyboardLayout layout;
    private Runnable onReleased;
    private KeyboardFocus focus;
    private boolean focusVisible = true;
    private final boolean[] padDown = new boolean[Pad.values().length];
    private KeyboardFocus.Direction padDirection, stickDirection, activeDirection;
    private long nextNavigate;

    public PcKeyboardController(PcKeyboardLayout layout, PcKeyboardEngine.Output output, Host host) {
        this.host = host;
        this.engine = new PcKeyboardEngine(output);
        this.layout = layout;
        this.focus = new KeyboardFocus(layout);
    }

    public PcKeyboardLayout layout() {
        return layout;
    }

    public PcKeyboardEngine engine() {
        return engine;
    }

    public KeyboardFocus focus() {
        return focus;
    }

    public boolean isFocusVisible() {
        return focusVisible;
    }

    /** Switch layouts; whatever was held is released first because its key no longer exists. */
    public void setLayout(PcKeyboardLayout newLayout) {
        if (newLayout == layout) return;
        release();
        layout = newLayout;
        focus = new KeyboardFocus(newLayout);
        host.changed();
    }

    /** Whether {@link #tick} must keep running: a key repeats or a direction is held. */
    public boolean needsTick() {
        return engine.hasHeldKeys() || activeDirection != null;
    }

    public void tick(long now) {
        engine.tick(now);
        if (activeDirection != null && now >= nextNavigate) {
            nextNavigate = now + NAVIGATE_REPEAT_MS;
            focus.move(activeDirection);
            host.changed();
        }
    }

    // Touch.

    public void touchDown(PcKey key, int pointerId, long now) {
        focusVisible = false;
        engine.press(key, pointerId, now);
        host.changed();
    }

    public void touchUp(int pointerId) {
        engine.release(pointerId);
        host.changed();
    }

    // Controller and remote.

    public void pad(Pad button, boolean down, long now) {
        if (padDown[button.ordinal()] == down) return;
        padDown[button.ordinal()] = down;
        switch (button) {
            case UP: padDirection(KeyboardFocus.Direction.UP, down, now); break;
            case DOWN: padDirection(KeyboardFocus.Direction.DOWN, down, now); break;
            case LEFT: padDirection(KeyboardFocus.Direction.LEFT, down, now); break;
            case RIGHT: padDirection(KeyboardFocus.Direction.RIGHT, down, now); break;
            case A:
                if (down) {
                    if (revealFocus()) break;
                    engine.press(focus.focused(), Pad.A, now);
                } else {
                    engine.release(Pad.A);
                }
                break;
            case B:
                if (down) host.close();
                return;
            case X: key("Backspace", Pad.X, down, now); break;
            case Y: key("Space", Pad.Y, down, now); break;
            case START: key("Enter", Pad.START, down, now); break;
            case LB:
                if (down) engine.tapModifier(PcModifier.SHIFT);
                break;
            default:
                if (down) engine.tapModifier(PcModifier.CTRL);
                break;
        }
        host.changed();
    }

    /**
     * The direction a left stick points (y growing downwards as Android reports it), or null while it is
     * centred. Pushing past 0.6 engages a direction; it holds until the stick falls back under 0.35.
     */
    public static KeyboardFocus.Direction stickDirection(float x, float y, KeyboardFocus.Direction current) {
        float ax = Math.abs(x), ay = Math.abs(y);
        float threshold = current == null ? STICK_ENGAGE : STICK_RELEASE;
        if (Math.max(ax, ay) < threshold) return null;
        if (ax > ay) return x < 0 ? KeyboardFocus.Direction.LEFT : KeyboardFocus.Direction.RIGHT;
        return y < 0 ? KeyboardFocus.Direction.UP : KeyboardFocus.Direction.DOWN;
    }

    /** The left stick, with y growing downwards as Android reports it. */
    public void stick(float x, float y, long now) {
        KeyboardFocus.Direction wanted = stickDirection(x, y, stickDirection);
        if (wanted == stickDirection) return;
        stickDirection = wanted;
        updateDirection(now);
    }

    /** Let go of every pad button held, as when the controller that held them is unplugged. */
    public void releasePads(long now) {
        for (Pad button : Pad.values()) {
            if (padDown[button.ordinal()]) pad(button, false, now);
        }
    }

    /** Told after {@link #release}, so whoever feeds this controller can resync what it believes is held. */
    public void setOnReleased(Runnable onReleased) {
        this.onReleased = onReleased;
    }

    /** Release everything pressed and forget locked modifiers, as when the keyboard closes. */
    public void release() {
        engine.releaseAll();
        java.util.Arrays.fill(padDown, false);
        padDirection = null;
        stickDirection = null;
        activeDirection = null;
        host.changed();
        if (onReleased != null) onReleased.run();
    }

    private void key(String id, Pad owner, boolean down, long now) {
        if (down) {
            PcKey key = layout.key(id);
            if (key != null) engine.press(key, owner, now);
        } else {
            engine.release(owner);
        }
    }

    private void padDirection(KeyboardFocus.Direction direction, boolean down, long now) {
        if (down) padDirection = direction;
        else if (padDirection == direction) padDirection = null;
        updateDirection(now);
    }

    private void updateDirection(long now) {
        KeyboardFocus.Direction wanted = padDirection != null ? padDirection : stickDirection;
        if (wanted == activeDirection) return;
        activeDirection = wanted;
        if (wanted == null) return;
        nextNavigate = now + NAVIGATE_DELAY_MS;
        // The first press only shows the selection when touch had hidden it.
        if (!revealFocus()) focus.move(wanted);
        host.changed();
    }

    /** Show the selection if hidden; true when this input did nothing else. */
    private boolean revealFocus() {
        if (focusVisible) return false;
        focusVisible = true;
        return true;
    }
}

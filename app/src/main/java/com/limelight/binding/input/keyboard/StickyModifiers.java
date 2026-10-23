package com.limelight.binding.input.keyboard;

import java.util.EnumMap;

/**
 * The state of the sticky modifier keys. Tapping a modifier arms it for the next key, tapping it
 * again locks it, and a third tap releases it. A key press uses up every armed modifier but leaves
 * the locked ones down.
 */
public final class StickyModifiers {
    public enum State { OFF, ONCE, LOCKED }

    private final EnumMap<PcModifier, State> states = new EnumMap<>(PcModifier.class);

    public StickyModifiers() {
        clear();
    }

    public State state(PcModifier modifier) {
        return states.get(modifier);
    }

    /** Whether the next key is typed with the modifier: armed or locked. */
    public boolean isActive(PcModifier modifier) {
        return states.get(modifier) != State.OFF;
    }

    /** Advance OFF to ONCE to LOCKED to OFF and return the new state. */
    public State tap(PcModifier modifier) {
        State next;
        switch (states.get(modifier)) {
            case OFF: next = State.ONCE; break;
            case ONCE: next = State.LOCKED; break;
            default: next = State.OFF; break;
        }
        states.put(modifier, next);
        return next;
    }

    /** Used by the engine when a key press has used an armed modifier up. */
    void set(PcModifier modifier, State state) {
        states.put(modifier, state);
    }

    public void clear() {
        for (PcModifier modifier : PcModifier.values()) states.put(modifier, State.OFF);
    }

    public boolean any() {
        for (State state : states.values()) {
            if (state != State.OFF) return true;
        }
        return false;
    }
}

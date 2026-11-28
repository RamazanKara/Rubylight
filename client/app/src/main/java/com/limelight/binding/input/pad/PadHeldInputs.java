package com.limelight.binding.input.pad;

import com.limelight.binding.input.HeldInputState;

/**
 * Zero-allocation primitive reference counter for on-screen keyboard virtual keys and mouse buttons
 * shared across {@link WasdKeys}, {@link PadTouchZoneHandler}, and {@link PadCustomButtonView}.
 */
public final class PadHeldInputs {
    private static final int KEY_CAPACITY = 256;
    private static final int MOUSE_CAPACITY = 6;

    private final HeldInputState.Sink sink;
    private final int[] keyRefs = new int[KEY_CAPACITY];
    private final int[] mouseRefs = new int[MOUSE_CAPACITY];
    private final WasdKeys.Keys wasdSink = this::key;

    public PadHeldInputs(HeldInputState.Sink sink) {
        this.sink = sink;
    }

    /** Returns the shared {@link WasdKeys.Keys} adapter without allocating per call. */
    WasdKeys.Keys asWasdKeys() {
        return wasdSink;
    }

    /** Reference-counts a Windows virtual-key press or release (0x01..0xFF). */
    public void key(int vk, boolean down) {
        if (vk < 1 || vk >= KEY_CAPACITY) return;
        if (down) {
            if (keyRefs[vk]++ == 0 && sink != null) {
                sink.key(vk, true);
            }
        } else {
            if (keyRefs[vk] <= 0) return;
            if (--keyRefs[vk] == 0 && sink != null) {
                sink.key(vk, false);
            }
        }
    }

    /** Reference-counts a mouse button press or release (1..5). */
    public void mouse(int button, boolean down) {
        if (button < 1 || button >= MOUSE_CAPACITY) return;
        if (down) {
            if (mouseRefs[button]++ == 0 && sink != null) {
                sink.mouse(button, true);
            }
        } else {
            if (mouseRefs[button] <= 0) return;
            if (--mouseRefs[button] == 0 && sink != null) {
                sink.mouse(button, false);
            }
        }
    }

    /** Presses {@code count} keys from {@code vks} in forward order. */
    public void pressKeys(int[] vks, int count) {
        if (vks == null) return;
        int limit = Math.min(count, vks.length);
        for (int i = 0; i < limit; i++) {
            key(vks[i], true);
        }
    }

    /** Releases {@code count} keys from {@code vks} in reverse order so non-modifiers lift first. */
    public void releaseKeys(int[] vks, int count) {
        if (vks == null) return;
        int limit = Math.min(count, vks.length);
        for (int i = limit - 1; i >= 0; i--) {
            key(vks[i], false);
        }
    }

    public int keyRefCount(int vk) {
        return vk >= 1 && vk < KEY_CAPACITY ? keyRefs[vk] : 0;
    }

    public int mouseRefCount(int button) {
        return button >= 1 && button < MOUSE_CAPACITY ? mouseRefs[button] : 0;
    }

    public boolean isKeyHeld(int vk) {
        return keyRefCount(vk) > 0;
    }

    public boolean isMouseHeld(int button) {
        return mouseRefCount(button) > 0;
    }

    /**
     * Deterministically releases all held keys and mouse buttons in ascending code order,
     * releasing non-modifier virtual keys before modifiers.
     */
    public void releaseAll() {
        for (int vk = 1; vk < KEY_CAPACITY; vk++) {
            if (keyRefs[vk] > 0 && !isModifierVk(vk)) {
                keyRefs[vk] = 0;
                if (sink != null) sink.key(vk, false);
            }
        }
        for (int vk = 1; vk < KEY_CAPACITY; vk++) {
            if (keyRefs[vk] > 0 && isModifierVk(vk)) {
                keyRefs[vk] = 0;
                if (sink != null) sink.key(vk, false);
            }
        }
        for (int button = 1; button < MOUSE_CAPACITY; button++) {
            if (mouseRefs[button] > 0) {
                mouseRefs[button] = 0;
                if (sink != null) sink.mouse(button, false);
            }
        }
    }

    static boolean isModifierVk(int vk) {
        return vk == 0x5B || vk == 0x5C || (vk >= 0xA0 && vk <= 0xA5);
    }
}

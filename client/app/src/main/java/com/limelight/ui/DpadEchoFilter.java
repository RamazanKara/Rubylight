package com.limelight.ui;

import android.util.SparseIntArray;
import android.view.KeyEvent;

/**
 * Drops the second copy of a D-pad press. Android turns an unhandled joystick hat (and the left stick)
 * into DPAD key events flagged {@link KeyEvent#FLAG_FALLBACK}, so a pad that also reports real DPAD keys
 * for the same press would move the focus twice. Whichever of the two arrives first owns the key until
 * it is released; the other copy, its repeats and its release are swallowed. A pad that reports only
 * one kind, a TV remote and a keyboard never see a difference.
 */
public final class DpadEchoFilter {
    private static final int REAL = 1, FALLBACK = 2;
    // Keyed by device and key code. held: which kind went down and has not come up. dropped: which kind
    // lost to the other, so its repeats and release are swallowed too.
    private final SparseIntArray held = new SparseIntArray();
    private final SparseIntArray dropped = new SparseIntArray();

    public static boolean isDpadKey(int keyCode) {
        return keyCode == KeyEvent.KEYCODE_DPAD_UP || keyCode == KeyEvent.KEYCODE_DPAD_DOWN
                || keyCode == KeyEvent.KEYCODE_DPAD_LEFT || keyCode == KeyEvent.KEYCODE_DPAD_RIGHT;
    }

    /** True when the event repeats a press already delivered in the other form and must not be dispatched. */
    public boolean drop(KeyEvent event) {
        int keyCode = event.getKeyCode();
        if (!isDpadKey(keyCode)) return false;
        int kind = (event.getFlags() & KeyEvent.FLAG_FALLBACK) != 0 ? FALLBACK : REAL;
        int slot = event.getDeviceId() * 256 + keyCode;
        int other = kind == REAL ? FALLBACK : REAL;
        if (event.getAction() == KeyEvent.ACTION_DOWN) {
            if ((dropped.get(slot) & kind) != 0) return true;
            if ((held.get(slot) & other) != 0) {
                dropped.put(slot, dropped.get(slot) | kind);
                return true;
            }
            held.put(slot, held.get(slot) | kind);
            return false;
        }
        if (event.getAction() == KeyEvent.ACTION_UP) {
            if ((dropped.get(slot) & kind) != 0) {
                clear(dropped, slot, kind);
                return true;
            }
            clear(held, slot, kind);
        }
        return false;
    }

    /** Forget everything held, as when the window loses focus and its releases will never arrive. */
    public void reset() {
        held.clear();
        dropped.clear();
    }

    private static void clear(SparseIntArray array, int slot, int kind) {
        int value = array.get(slot) & ~kind;
        if (value == 0) array.delete(slot);
        else array.put(slot, value);
    }
}

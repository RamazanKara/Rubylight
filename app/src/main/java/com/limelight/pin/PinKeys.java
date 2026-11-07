package com.limelight.pin;

import android.view.KeyEvent;

/**
 * What a key does on the PIN pad beyond moving over it, which focus does: a remote's or a keyboard's digits type, Delete and
 * Enter work, and a pad's X and B take back and leave. Pure, so the table can be tested.
 */
final class PinKeys {
    enum Kind { NONE, DIGIT, DELETE, BACK, SUBMIT }

    /** One decision. {@code value} is the digit of a DIGIT. */
    static final class Action {
        final Kind kind;
        final int value;

        Action(Kind kind, int value) {
            this.kind = kind;
            this.value = value;
        }

        @Override public boolean equals(Object o) {
            return o instanceof Action && ((Action) o).kind == kind && ((Action) o).value == value;
        }

        @Override public int hashCode() {
            return kind.ordinal() * 31 + value;
        }

        @Override public String toString() {
            return kind + "(" + value + ")";
        }
    }

    static final Action NONE = new Action(Kind.NONE, 0);

    private PinKeys() {
    }

    /**
     * @param typedOnKeyboard the event comes from a full keyboard, where Enter means "done"; on a remote the
     *                        same code is its Select button
     */
    static Action of(int keyCode, boolean typedOnKeyboard) {
        switch (keyCode) {
            case KeyEvent.KEYCODE_ENTER: return typedOnKeyboard ? new Action(Kind.SUBMIT, 0) : NONE;
            case KeyEvent.KEYCODE_NUMPAD_ENTER:
            case KeyEvent.KEYCODE_BUTTON_START: return new Action(Kind.SUBMIT, 0);
            case KeyEvent.KEYCODE_DEL:
            case KeyEvent.KEYCODE_FORWARD_DEL:
            case KeyEvent.KEYCODE_BUTTON_X: return new Action(Kind.DELETE, 0);
            case KeyEvent.KEYCODE_BUTTON_B:
            case KeyEvent.KEYCODE_ESCAPE: return new Action(Kind.BACK, 0);
            default:
                if (keyCode >= KeyEvent.KEYCODE_0 && keyCode <= KeyEvent.KEYCODE_9) {
                    return new Action(Kind.DIGIT, keyCode - KeyEvent.KEYCODE_0);
                }
                if (keyCode >= KeyEvent.KEYCODE_NUMPAD_0 && keyCode <= KeyEvent.KEYCODE_NUMPAD_9) {
                    return new Action(Kind.DIGIT, keyCode - KeyEvent.KEYCODE_NUMPAD_0);
                }
                return NONE;
        }
    }
}

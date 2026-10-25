package com.limelight.pin;

import android.view.KeyEvent;

/**
 * What a key does on the PIN pad. A remote or a game pad types the highlighted row the way Google TV's PIN
 * entry does: Up and Down pick the row, Left, Select and Right type its first, second and third key. Number
 * keys, Delete and Enter work from a keyboard. Pure, so the table can be tested.
 */
final class PinKeys {
    enum Kind { NONE, ROW_UP, ROW_DOWN, CELL, DIGIT, DELETE, BACK, SUBMIT }

    /** One decision. {@code value} is the column of a CELL or the digit of a DIGIT. */
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

    /** The pad: three keys a row; -1 is Delete and -2 is Confirm. */
    static final int DELETE = -1, CONFIRM = -2;
    static final int[][] ROWS = {{1, 2, 3}, {4, 5, 6}, {7, 8, 9}, {DELETE, 0, CONFIRM}};

    private PinKeys() {
    }

    /**
     * @param typedOnKeyboard the event comes from a full keyboard, where Enter means "done"; on a remote the
     *                        same code is its Select button
     */
    static Action of(int keyCode, boolean typedOnKeyboard) {
        switch (keyCode) {
            case KeyEvent.KEYCODE_DPAD_UP: return new Action(Kind.ROW_UP, 0);
            case KeyEvent.KEYCODE_DPAD_DOWN: return new Action(Kind.ROW_DOWN, 0);
            case KeyEvent.KEYCODE_DPAD_LEFT: return new Action(Kind.CELL, 0);
            case KeyEvent.KEYCODE_DPAD_CENTER:
            case KeyEvent.KEYCODE_BUTTON_A: return new Action(Kind.CELL, 1);
            case KeyEvent.KEYCODE_ENTER: return typedOnKeyboard ? new Action(Kind.SUBMIT, 0) : new Action(Kind.CELL, 1);
            case KeyEvent.KEYCODE_DPAD_RIGHT: return new Action(Kind.CELL, 2);
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

    /** The pad key that Up or Down lands on, wrapping at the ends. */
    static int moveRow(int row, boolean down) {
        return (row + (down ? 1 : ROWS.length - 1)) % ROWS.length;
    }

    /** What the key at {@code row} and {@code column} does: a digit, {@link #DELETE} or {@link #CONFIRM}. */
    static int cell(int row, int column) {
        return ROWS[row][column];
    }
}

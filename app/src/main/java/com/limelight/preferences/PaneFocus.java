package com.limelight.preferences;

import java.util.function.IntPredicate;

/** Which row of the right-hand settings pane takes focus when the D-pad enters it. */
final class PaneFocus {
    private PaneFocus() {}

    /**
     * The row the pane was left on if it is still there and can be focused, else its first focusable row
     * (category headings and disabled-for-focus rows are skipped), or -1 for an empty pane.
     */
    static int target(int remembered, int count, IntPredicate focusable) {
        if (remembered >= 0 && remembered < count && focusable.test(remembered)) return remembered;
        for (int i = 0; i < count; i++) {
            if (focusable.test(i)) return i;
        }
        return -1;
    }
}

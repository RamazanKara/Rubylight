package com.limelight.preferences;

import static org.junit.Assert.*;

import org.junit.Test;

public class PaneFocusTest {
    // Row 0 is a category heading, row 3 is disabled.
    private static boolean focusable(int row) { return row != 0 && row != 3; }

    @Test
    public void aFirstEntryLandsOnTheFirstFocusableRow() {
        assertEquals(1, PaneFocus.target(-1, 6, PaneFocusTest::focusable));
    }

    @Test
    public void returningToASectionLandsOnTheRowItWasLeftOn() {
        assertEquals(4, PaneFocus.target(4, 6, PaneFocusTest::focusable));
    }

    @Test
    public void aRememberedRowThatIsGoneOrNotFocusableFallsBackToTheFirst() {
        assertEquals(1, PaneFocus.target(3, 6, PaneFocusTest::focusable));
        assertEquals(1, PaneFocus.target(0, 6, PaneFocusTest::focusable));
        assertEquals(1, PaneFocus.target(9, 6, PaneFocusTest::focusable));
    }

    @Test
    public void anEmptyPaneHasNoTarget() {
        assertEquals(-1, PaneFocus.target(2, 0, PaneFocusTest::focusable));
        assertEquals(-1, PaneFocus.target(-1, 3, row -> false));
    }
}

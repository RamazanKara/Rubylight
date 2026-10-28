package com.limelight.ui;

import org.junit.Test;

import static org.junit.Assert.*;

public class HostKeyboardTest {
    private final HostKeyboard keyboard = new HostKeyboard();

    @Test
    public void aRaiseOpensTheKeyboardOnlyRightAfterTheUsersOwnInput() {
        keyboard.onFocus(0);
        assertFalse("a game field that takes the focus by itself", keyboard.mayOpen(10_000));

        keyboard.onPress(0.5f, 10_000);
        assertTrue(keyboard.mayOpen(10_000 + HostKeyboard.INPUT_WINDOW_MS));
        assertFalse("too long after the tap", keyboard.mayOpen(10_001 + HostKeyboard.INPUT_WINDOW_MS));
        assertFalse("a clock that went back is no proof", keyboard.mayOpen(9_999));

        keyboard.onInput(20_000);
        assertTrue("a key counts like a tap", keyboard.mayOpen(20_500));
    }

    @Test
    public void noFieldNoKeyboard() {
        keyboard.onPress(0.5f, 1_000);
        assertFalse(keyboard.mayOpen(1_000));
        keyboard.onFocus(0);
        keyboard.onLower();
        assertFalse("the field lost the focus", keyboard.mayOpen(1_000));
    }

    @Test
    public void aKeyboardTheUserClosedStaysClosedForThatFieldOnly() {
        keyboard.onPress(0.5f, 1_000);
        keyboard.onFocus(0);
        assertTrue(keyboard.mayOpen(1_100));
        keyboard.onOpened();
        keyboard.onVisibility(true, 1_200);

        // The user closes it, then taps on the stream again: the same field does not reopen it.
        assertTrue(keyboard.onVisibility(false, 2_000));
        keyboard.onPress(0.5f, 2_500);
        assertTrue(keyboard.dismissedHere());
        assertFalse(keyboard.mayOpen(2_600));

        // The focus moves to another field (an update) right after a tap: it opens again.
        keyboard.onFocus(0);
        assertFalse(keyboard.dismissedHere());
        assertTrue(keyboard.mayOpen(2_600));
    }

    @Test
    public void withElementIdsAClosedKeyboardStaysClosedForThatFieldEvenWhenTheFocusComesBack() {
        int x = 0x1234_5678, y = 0x8000_0001;
        keyboard.onPress(0.5f, 1_000);
        keyboard.onFocus(x);
        assertTrue(keyboard.mayOpen(1_100));
        keyboard.onOpened();
        keyboard.onVisibility(true, 1_200);
        assertTrue(keyboard.onVisibility(false, 2_000));

        // The host repeats the field (an update for the same element): still closed.
        keyboard.onFocus(x);
        keyboard.onPress(0.5f, 2_500);
        assertTrue(keyboard.dismissedHere());
        assertFalse(keyboard.mayOpen(2_600));

        // Another field, an id with the top bit set: it opens.
        keyboard.onFocus(y);
        assertFalse(keyboard.dismissedHere());
        assertTrue(keyboard.mayOpen(2_600));

        // Back to the first, by an update or after a lower: closed for it still.
        keyboard.onFocus(x);
        assertTrue(keyboard.dismissedHere());
        assertFalse(keyboard.mayOpen(2_600));
        keyboard.onLower();
        keyboard.onPress(0.5f, 3_000);
        keyboard.onFocus(x);
        assertFalse(keyboard.mayOpen(3_100));

        // A new connection forgets it.
        keyboard.reset();
        keyboard.onPress(0.5f, 4_000);
        keyboard.onFocus(x);
        assertTrue(keyboard.mayOpen(4_100));
    }

    @Test
    public void withoutElementIdsEveryRaiseIsAnotherField() {
        keyboard.onPress(0.5f, 1_000);
        keyboard.onFocus(0);
        keyboard.onVisibility(true, 1_100);
        keyboard.onVisibility(false, 1_200);
        assertTrue(keyboard.dismissedHere());
        // An older host's raise for the same field cannot be told from another one.
        keyboard.onLower();
        keyboard.onPress(0.5f, 2_000);
        keyboard.onFocus(0);
        assertTrue(keyboard.mayOpen(2_100));
    }

    @Test
    public void closingTheKeyboardTheHostOpenedMakesTheNextOneTheUsers() {
        keyboard.onPress(0.5f, 1_000);
        keyboard.onFocus(0);
        keyboard.onOpened();
        keyboard.onVisibility(true, 1_100);
        assertTrue(keyboard.hostOpened());

        keyboard.onVisibility(false, 2_000);
        assertFalse(keyboard.hostOpened());
        // Opened again by hand: the field's lower leaves it open.
        keyboard.onVisibility(true, 3_000);
        assertFalse(keyboard.onLower());
    }

    @Test
    public void theLowerClosesTheKeyboardTheHostOpenedAndIsNoDismissal() {
        keyboard.onPress(0.5f, 1_000);
        keyboard.onFocus(0);
        keyboard.onOpened();
        keyboard.onVisibility(true, 1_100);
        assertTrue(keyboard.onLower());
        keyboard.onVisibility(false, 1_200);

        keyboard.onPress(0.5f, 5_000);
        keyboard.onFocus(0);
        assertTrue(keyboard.mayOpen(5_100));
        assertFalse("the lower already gave up ownership", keyboard.onLower());
    }

    @Test
    public void theStreamMovesUpSoThePressStaysAboveTheKeyboardAndBackWhenItCloses() {
        keyboard.onPress(0.75f, 1_000);
        keyboard.onFocus(0);
        assertEquals("closed: no move", 0, keyboard.pan(2400, 1000, 0, 2400, 24), 0);
        keyboard.onVisibility(true, 1_300);
        // The press at 1800 of 2400, the keyboard's top edge at 1400, 24 px of room.
        assertEquals(-424, keyboard.pan(2400, 1000, 0, 2400, 24), 0);
        // The stream laid out lower (letterboxed) moves by as much more.
        assertEquals(-524, keyboard.pan(2400, 1000, 100, 2400, 24), 0);

        keyboard.onVisibility(false, 2_000);
        assertEquals(0, keyboard.pan(2400, 1000, 0, 2400, 24), 0);
    }

    @Test
    public void aPressAboveTheKeyboardOrAFloatingKeyboardMovesNothing() {
        keyboard.onPress(0.25f, 1_000);
        keyboard.onFocus(0);
        keyboard.onVisibility(true, 1_100);
        assertEquals(0, keyboard.pan(2400, 1000, 0, 2400, 24), 0);
        assertEquals("a floating keyboard covers no bottom", 0, keyboard.pan(2400, 0, 0, 2400, 24), 0);
    }

    @Test
    public void theTargetIsLockedForOneKeyboardAndNeedsAFreshPressIntoAHostField() {
        keyboard.onPress(0.75f, 1_000);
        keyboard.onFocus(0);
        keyboard.onVisibility(true, 1_000 + HostKeyboard.PRESS_MAX_AGE_MS + 1);
        assertEquals("the press is stale when the keyboard opens", 0, keyboard.pan(2400, 1000, 0, 2400, 24), 0);
        keyboard.onVisibility(false, 7_000);

        keyboard.onPress(0.75f, 8_000);
        keyboard.onVisibility(true, 8_100);
        float pan = keyboard.pan(2400, 1000, 0, 2400, 24);
        assertEquals(-424, pan, 0);
        // A press while it is open does not move the stream again.
        keyboard.onPress(0.95f, 8_500);
        assertEquals(pan, keyboard.pan(2400, 1000, 0, 2400, 24), 0);

        // No host field: a keyboard opened by hand covers the stream as before.
        keyboard.onLower();
        keyboard.onVisibility(false, 9_000);
        keyboard.onPress(0.75f, 9_500);
        keyboard.onVisibility(true, 9_600);
        assertEquals(0, keyboard.pan(2400, 1000, 0, 2400, 24), 0);
    }

    @Test
    public void aPressWithoutAPointStillCountsAsInputButMovesNothing() {
        keyboard.onPress(Float.NaN, 1_000);
        keyboard.onFocus(0);
        assertTrue(keyboard.mayOpen(1_100));
        keyboard.onVisibility(true, 1_200);
        assertEquals(0, keyboard.pan(2400, 1000, 0, 2400, 24), 0);
    }

    @Test
    public void aNewConnectionForgetsTheFieldTheInputAndTheTarget() {
        keyboard.onPress(0.75f, 1_000);
        keyboard.onFocus(0);
        keyboard.onOpened();
        keyboard.onVisibility(true, 1_100);
        keyboard.reset();
        assertFalse(keyboard.hostOpened());
        assertEquals(0, keyboard.pan(2400, 1000, 0, 2400, 24), 0);
        keyboard.onFocus(0);
        assertFalse("input before the reset does not count", keyboard.mayOpen(1_200));
        assertTrue(keyboard.visible());
    }

    @Test
    public void theOffsetIsVPlusViewportOffset() {
        assertEquals(-50, HostKeyboard.offset(1000, 974, 24), 0);
        assertEquals(0, HostKeyboard.offset(100, 974, 24), 0);
    }
}

package com.limelight.binding.input.keyboard;

import static com.limelight.binding.input.keyboard.PcKeyboardController.NAVIGATE_DELAY_MS;
import static com.limelight.binding.input.keyboard.PcKeyboardController.NAVIGATE_REPEAT_MS;
import static com.limelight.binding.input.keyboard.PcKeyboardController.Pad;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

import java.util.ArrayList;
import java.util.List;

public class PcKeyboardControllerTest {
    private final List<String> events = new ArrayList<>();
    private int closed, changed;
    private final PcKeyboardController controller = new PcKeyboardController(
            PcKeyboardLayout.byId(PcKeyboardLayout.US),
            (vk, down, modifiers) -> events.add(Integer.toHexString(vk).toUpperCase() + (down ? "+" : "-")),
            new PcKeyboardController.Host() {
                @Override public void close() { closed++; }
                @Override public void changed() { changed++; }
            });

    private String focused() {
        return controller.focus().focused().id;
    }

    @Test
    public void dpadMovesTheSelectionOneKeyPerPress() {
        assertEquals("G", focused());
        controller.pad(Pad.RIGHT, true, 0);
        controller.pad(Pad.RIGHT, false, 10);
        assertEquals("H", focused());
        controller.pad(Pad.LEFT, true, 20);
        controller.pad(Pad.LEFT, false, 30);
        controller.pad(Pad.LEFT, true, 40);
        controller.pad(Pad.LEFT, false, 50);
        assertEquals("F", focused());
    }

    @Test
    public void aHeldDirectionRepeatsAfterTheDelayAtASteadyPace() {
        controller.pad(Pad.RIGHT, true, 1000);
        assertEquals("H", focused());
        controller.tick(1000 + NAVIGATE_DELAY_MS - 1);
        assertEquals("H", focused());
        controller.tick(1000 + NAVIGATE_DELAY_MS);
        assertEquals("J", focused());
        controller.tick(1000 + NAVIGATE_DELAY_MS + NAVIGATE_REPEAT_MS - 1);
        assertEquals("J", focused());
        controller.tick(1000 + NAVIGATE_DELAY_MS + NAVIGATE_REPEAT_MS);
        assertEquals("K", focused());
        controller.pad(Pad.RIGHT, false, 5000);
        assertFalse(controller.needsTick());
        controller.tick(9000);
        assertEquals("K", focused());
    }

    @Test
    public void theLeftStickNavigatesWithHysteresisAndTheDpadWins() {
        controller.stick(0.3f, 0, 0);
        assertEquals("G", focused());
        controller.stick(0.8f, 0.1f, 0);
        assertEquals("H", focused());
        // Easing a little does not release or re-trigger the direction.
        controller.stick(0.5f, 0, 100);
        controller.stick(0.8f, 0, 200);
        assertEquals("H", focused());
        controller.stick(0.1f, 0, 300);
        controller.stick(0, -0.9f, 400);
        assertEquals("Y", focused());
        controller.pad(Pad.DOWN, true, 500);
        assertEquals("H", focused());
        controller.stick(0, 0, 600);
        controller.pad(Pad.DOWN, false, 700);
        assertFalse(controller.needsTick());
    }

    @Test
    public void aPressesTheSelectedKeyAndHoldingItRepeats() {
        controller.pad(Pad.A, true, 0);
        assertEquals(List.of("47+"), events);
        controller.tick(PcKeyboardEngine.REPEAT_DELAY_MS);
        assertEquals(List.of("47+", "47+"), events);
        controller.pad(Pad.A, false, 500);
        assertEquals("47-", events.get(events.size() - 1));
    }

    @Test
    public void faceButtonsAreBackspaceSpaceAndEnterAndBCloses() {
        controller.pad(Pad.X, true, 0);
        controller.pad(Pad.X, false, 1);
        controller.pad(Pad.Y, true, 2);
        controller.pad(Pad.Y, false, 3);
        controller.pad(Pad.START, true, 4);
        controller.pad(Pad.START, false, 5);
        assertEquals(List.of("8+", "8-", "20+", "20-", "D+", "D-"), events);
        assertEquals(0, closed);
        controller.pad(Pad.B, true, 6);
        assertEquals(1, closed);
        controller.pad(Pad.B, false, 7);
        assertEquals(1, closed);
    }

    @Test
    public void bumpersCycleShiftAndCtrlLikeTheKeys() {
        controller.pad(Pad.LB, true, 0);
        controller.pad(Pad.LB, false, 1);
        assertEquals(StickyModifiers.State.ONCE, controller.engine().modifiers().state(PcModifier.SHIFT));
        controller.pad(Pad.LB, true, 2);
        controller.pad(Pad.LB, false, 3);
        assertEquals(StickyModifiers.State.LOCKED, controller.engine().modifiers().state(PcModifier.SHIFT));
        controller.pad(Pad.RB, true, 4);
        controller.pad(Pad.RB, false, 5);
        assertEquals(StickyModifiers.State.ONCE, controller.engine().modifiers().state(PcModifier.CTRL));
        controller.pad(Pad.LB, true, 6);
        controller.pad(Pad.LB, false, 7);
        assertEquals(StickyModifiers.State.OFF, controller.engine().modifiers().state(PcModifier.SHIFT));
    }

    @Test
    public void aTouchHidesTheSelectionAndTheNextPadInputShowsItAndActs() {
        assertTrue(controller.isFocusVisible());
        controller.touchDown(controller.layout().key("Q"), 7, 0);
        controller.touchUp(7);
        assertFalse(controller.isFocusVisible());
        assertEquals("the selection follows the finger", "Q", focused());
        events.clear();
        controller.pad(Pad.RIGHT, true, 10);
        controller.pad(Pad.RIGHT, false, 11);
        assertTrue(controller.isFocusVisible());
        assertEquals("the first press already moved from the touched key", "W", focused());
    }

    @Test
    public void theFirstAPressAfterTouchTypesTheTouchedKey() {
        controller.touchDown(controller.layout().key("Q"), 7, 0);
        controller.touchUp(7);
        events.clear();
        controller.pad(Pad.A, true, 20);
        controller.pad(Pad.A, false, 21);
        assertTrue(controller.isFocusVisible());
        assertEquals(2, events.size());
        assertTrue(events.get(0).endsWith("+"));
        assertTrue(events.get(1).endsWith("-"));
    }

    @Test
    public void closingWithControllerAndTouchKeysDownReleasesAll() {
        controller.pad(Pad.LB, true, 0);
        controller.pad(Pad.LB, false, 1);
        controller.pad(Pad.LB, true, 2);
        controller.pad(Pad.LB, false, 3);
        controller.pad(Pad.A, true, 4);
        controller.pad(Pad.X, true, 5);
        controller.touchDown(controller.layout().key("W"), 3, 6);
        controller.pad(Pad.RIGHT, true, 7);
        controller.release();
        assertFalse(controller.needsTick());
        assertFalse(controller.engine().hasHeldKeys());
        assertFalse(controller.engine().modifiers().any());
        int downs = 0, ups = 0;
        for (String event : events) {
            if (event.endsWith("+")) downs++; else ups++;
        }
        assertEquals(events.toString(), downs, ups);
    }

    @Test
    public void switchingLayoutReleasesWhatWasHeld() {
        controller.touchDown(controller.layout().key("W"), 1, 0);
        controller.setLayout(PcKeyboardLayout.byId(PcKeyboardLayout.DE));
        assertEquals(List.of("57+", "57-"), events);
        assertEquals("DE", controller.layout().id.toUpperCase());
        assertEquals("G", focused());
    }

    @Test
    public void releasingThePadsAlsoLetsGoOfAHeldStickOrHat() {
        controller.stick(1, 0, 0);
        assertTrue(controller.stickActive());
        assertTrue(controller.needsTick());
        controller.releasePads(100);
        assertFalse(controller.stickActive());
        assertFalse("The release never comes once the pad is gone", controller.needsTick());
        String before = focused();
        controller.tick(10_000);
        assertEquals(before, focused());
    }
}

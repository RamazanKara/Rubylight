package com.limelight.binding.input.keyboard;

import static com.limelight.binding.input.keyboard.KeyboardFocus.Direction.DOWN;
import static com.limelight.binding.input.keyboard.KeyboardFocus.Direction.LEFT;
import static com.limelight.binding.input.keyboard.KeyboardFocus.Direction.RIGHT;
import static com.limelight.binding.input.keyboard.KeyboardFocus.Direction.UP;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

import java.util.List;

public class KeyboardFocusTest {
    private final PcKeyboardLayout us = PcKeyboardLayout.byId(PcKeyboardLayout.US);
    private final PcKeyboardLayout de = PcKeyboardLayout.byId(PcKeyboardLayout.DE);

    @Test
    public void startsOnTheHomeRow() {
        assertEquals("G", new KeyboardFocus(us).focused().id);
    }

    @Test
    public void leftAndRightWrapAroundTheRow() {
        KeyboardFocus focus = new KeyboardFocus(us);
        focus.focus("Esc");
        focus.move(LEFT);
        assertEquals("Pause", focus.focused().id);
        focus.move(RIGHT);
        assertEquals("Esc", focus.focused().id);
    }

    @Test
    public void upAndDownStopAtTheFirstAndLastRow() {
        KeyboardFocus focus = new KeyboardFocus(us);
        focus.focus("F5");
        focus.move(UP);
        assertEquals(0, focus.row());
        assertEquals("F5", focus.focused().id);
        focus.focus("Space");
        focus.move(DOWN);
        assertEquals("Space", focus.focused().id);
    }

    @Test
    public void verticalMovesLandOnTheNearestKeyAndComeBack() {
        KeyboardFocus focus = new KeyboardFocus(us);
        focus.focus("G");
        focus.move(DOWN);
        assertEquals("V", focus.focused().id);
        focus.move(DOWN);
        assertEquals("Space", focus.focused().id);
        focus.move(UP);
        assertEquals("V", focus.focused().id);
        focus.move(UP);
        assertEquals("G", focus.focused().id);
        focus.move(UP);
        assertEquals("T", focus.focused().id);
        focus.move(UP);
        assertEquals("6", focus.focused().id);
    }

    @Test
    public void theAnchorSurvivesARowWithAGap() {
        KeyboardFocus focus = new KeyboardFocus(us);
        focus.focus("Up");
        focus.move(DOWN);
        assertEquals("Down", focus.focused().id);
        focus.move(UP);
        assertEquals("Up", focus.focused().id);
        focus.focus("Left");
        focus.move(UP);
        assertEquals("Up", focus.focused().id);
    }

    @Test
    public void everyKeyIsReachableFromGWithTheDpad() {
        for (PcKeyboardLayout layout : new PcKeyboardLayout[] {us, de}) {
            java.util.Set<String> seen = new java.util.HashSet<>();
            java.util.ArrayDeque<String> queue = new java.util.ArrayDeque<>();
            queue.add("G");
            seen.add("G");
            while (!queue.isEmpty()) {
                String id = queue.poll();
                for (KeyboardFocus.Direction direction : KeyboardFocus.Direction.values()) {
                    KeyboardFocus focus = new KeyboardFocus(layout);
                    focus.focus(id);
                    focus.move(direction);
                    if (seen.add(focus.focused().id)) queue.add(focus.focused().id);
                }
            }
            int total = 0;
            for (List<PcKey> row : layout.rows()) total += row.size();
            assertEquals(layout.id, total, seen.size());
        }
    }

    @Test
    public void focusingAnUnknownKeyKeepsTheSelection() {
        KeyboardFocus focus = new KeyboardFocus(us);
        assertFalse(focus.focus("Nope"));
        assertEquals("G", focus.focused().id);
        assertTrue(focus.focus("Enter"));
    }
}

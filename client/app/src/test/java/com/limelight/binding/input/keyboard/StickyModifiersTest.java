package com.limelight.binding.input.keyboard;

import static com.limelight.binding.input.keyboard.StickyModifiers.State.LOCKED;
import static com.limelight.binding.input.keyboard.StickyModifiers.State.OFF;
import static com.limelight.binding.input.keyboard.StickyModifiers.State.ONCE;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public class StickyModifiersTest {
    @Test
    public void tapOnceArmsTapTwiceLocksTapThriceReleases() {
        StickyModifiers modifiers = new StickyModifiers();
        assertEquals(OFF, modifiers.state(PcModifier.SHIFT));
        assertFalse(modifiers.isActive(PcModifier.SHIFT));
        assertEquals(ONCE, modifiers.tap(PcModifier.SHIFT));
        assertTrue(modifiers.isActive(PcModifier.SHIFT));
        assertEquals(LOCKED, modifiers.tap(PcModifier.SHIFT));
        assertTrue(modifiers.isActive(PcModifier.SHIFT));
        assertEquals(OFF, modifiers.tap(PcModifier.SHIFT));
        assertFalse(modifiers.isActive(PcModifier.SHIFT));
        assertEquals(ONCE, modifiers.tap(PcModifier.SHIFT));
    }

    @Test
    public void modifiersAreIndependent() {
        StickyModifiers modifiers = new StickyModifiers();
        modifiers.tap(PcModifier.CTRL);
        modifiers.tap(PcModifier.ALT);
        modifiers.tap(PcModifier.ALT);
        assertEquals(ONCE, modifiers.state(PcModifier.CTRL));
        assertEquals(LOCKED, modifiers.state(PcModifier.ALT));
        assertEquals(OFF, modifiers.state(PcModifier.WIN));
        assertEquals(OFF, modifiers.state(PcModifier.ALTGR));
        assertTrue(modifiers.any());
    }

    @Test
    public void clearResetsEverything() {
        StickyModifiers modifiers = new StickyModifiers();
        for (PcModifier modifier : PcModifier.values()) {
            modifiers.tap(modifier);
            modifiers.tap(modifier);
        }
        modifiers.clear();
        assertFalse(modifiers.any());
        for (PcModifier modifier : PcModifier.values()) assertEquals(OFF, modifiers.state(modifier));
    }
}

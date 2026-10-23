package com.limelight.binding.input.keyboard;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import com.limelight.nvstream.input.KeyboardPacket;

import org.junit.Test;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

public class PcKeyboardEngineTest {
    private final List<String> events = new ArrayList<>();
    private final List<Byte> modifierBits = new ArrayList<>();
    private final PcKeyboardLayout layout = PcKeyboardLayout.byId(PcKeyboardLayout.US);
    private final PcKeyboardEngine engine = new PcKeyboardEngine((vk, down, modifiers) -> {
        events.add(Integer.toHexString(vk).toUpperCase() + (down ? "+" : "-"));
        modifierBits.add(modifiers);
    });

    private PcKey key(String id) {
        return layout.key(id);
    }

    private static final String SHIFT = "A0", CTRL = "A2", ALT = "A4", A = "41", B = "42";

    /** Every key down has been followed by its up. */
    private void assertNothingStuck() {
        Map<String, Integer> down = new HashMap<>();
        for (String event : events) {
            String vk = event.substring(0, event.length() - 1);
            int now = down.getOrDefault(vk, 0);
            down.put(vk, event.endsWith("+") ? 1 : 0);
            assertTrue(event, now == 1 || event.endsWith("+"));
        }
        for (Map.Entry<String, Integer> entry : down.entrySet()) {
            assertEquals("key " + entry.getKey() + " left down: " + events, 0, (int) entry.getValue());
        }
    }

    @Test
    public void aTouchIsKeyDownThenKeyUpSoGamesSeeItHeld() {
        engine.press(key("A"), 1, 0);
        assertEquals(List.of(A + "+"), events);
        engine.release(1);
        assertEquals(List.of(A + "+", A + "-"), events);
    }

    @Test
    public void anArmedModifierIsHeldForExactlyOneKey() {
        engine.tapModifier(PcModifier.SHIFT);
        assertTrue("nothing reaches the host until a key is pressed", events.isEmpty());
        engine.press(key("A"), 1, 0);
        engine.release(1);
        assertEquals(List.of(SHIFT + "+", A + "+", A + "-", SHIFT + "-"), events);
        assertEquals(KeyboardPacket.MODIFIER_SHIFT, (byte) modifierBits.get(1));
        assertFalse(engine.modifiers().isActive(PcModifier.SHIFT));

        events.clear();
        engine.press(key("B"), 1, 10);
        engine.release(1);
        assertEquals(List.of(B + "+", B + "-"), events);
    }

    @Test
    public void aLockedModifierStaysDownUntilTappedAgain() {
        engine.tapModifier(PcModifier.CTRL);
        engine.tapModifier(PcModifier.CTRL);
        assertEquals(List.of(CTRL + "+"), events);
        for (String id : new String[] {"C", "V"}) {
            engine.press(key(id), 1, 0);
            engine.release(1);
        }
        assertEquals(List.of(CTRL + "+", "43+", "43-", "56+", "56-"), events);
        assertEquals(StickyModifiers.State.LOCKED, engine.modifiers().state(PcModifier.CTRL));
        engine.tapModifier(PcModifier.CTRL);
        assertEquals(CTRL + "-", events.get(events.size() - 1));
        assertEquals(StickyModifiers.State.OFF, engine.modifiers().state(PcModifier.CTRL));
        assertNothingStuck();
    }

    @Test
    public void tappingTheModifierKeysUsesTheSameStateMachine() {
        engine.press(key("ShiftL"), 1, 0);
        engine.release(1);
        assertEquals(StickyModifiers.State.ONCE, engine.modifiers().state(PcModifier.SHIFT));
        engine.press(key("ShiftR"), 2, 0);
        assertEquals(StickyModifiers.State.LOCKED, engine.modifiers().state(PcModifier.SHIFT));
        engine.press(key("ShiftL"), 3, 0);
        assertEquals(StickyModifiers.State.OFF, engine.modifiers().state(PcModifier.SHIFT));
        assertNothingStuck();
    }

    @Test
    public void severalModifiersTogetherReleaseInTheirOwnTime() {
        engine.tapModifier(PcModifier.CTRL);
        engine.tapModifier(PcModifier.ALT);
        engine.press(key("Del"), 1, 0);
        byte bits = KeyboardPacket.MODIFIER_CTRL | KeyboardPacket.MODIFIER_ALT;
        assertEquals(bits, (byte) modifierBits.get(modifierBits.size() - 1));
        engine.release(1);
        assertNothingStuck();
        assertFalse(engine.modifiers().any());
    }

    @Test
    public void aHeldKeyRepeatsAfterTheDelayAndOnlyWhileHeld() {
        engine.press(key("A"), 1, 1000);
        engine.tick(1000 + PcKeyboardEngine.REPEAT_DELAY_MS - 1);
        assertEquals(1, events.size());
        engine.tick(1000 + PcKeyboardEngine.REPEAT_DELAY_MS);
        assertEquals(List.of(A + "+", A + "+"), events);
        engine.tick(1000 + PcKeyboardEngine.REPEAT_DELAY_MS + PcKeyboardEngine.REPEAT_MS - 1);
        assertEquals(2, events.size());
        engine.tick(1000 + PcKeyboardEngine.REPEAT_DELAY_MS + PcKeyboardEngine.REPEAT_MS);
        assertEquals(3, events.size());
        engine.release(1);
        engine.tick(100000);
        assertEquals(4, events.size());
        assertFalse(engine.hasHeldKeys());
    }

    @Test
    public void keysThatDoNotRepeatNeverDo() {
        engine.press(key("Caps"), 1, 0);
        engine.press(key("Esc"), 2, 0);
        engine.tick(100000);
        assertEquals(List.of("14+", "1B+"), events);
        engine.releaseAll();
        assertNothingStuck();
    }

    @Test
    public void twoFingersHoldTwoKeysAndTheSameFingerCannotPressTwice() {
        engine.press(key("W"), 1, 0);
        engine.press(key("D"), 2, 0);
        engine.press(key("D"), 2, 0);
        assertEquals(List.of("57+", "44+"), events);
        engine.release(1);
        engine.release(2);
        assertNothingStuck();
    }

    @Test
    public void closingReleasesHeldKeysLockedAndArmedModifiers() {
        engine.tapModifier(PcModifier.CTRL);
        engine.tapModifier(PcModifier.CTRL);
        engine.tapModifier(PcModifier.SHIFT);
        engine.tapModifier(PcModifier.ALT);
        engine.tapModifier(PcModifier.ALT);
        engine.press(key("A"), 1, 0);
        engine.press(key("Space"), 2, 0);
        engine.releaseAll();
        assertNothingStuck();
        assertFalse(engine.hasHeldKeys());
        assertFalse(engine.modifiers().any());
        // The packets of the releases carry no modifier that was already let go.
        assertEquals(0, (byte) modifierBits.get(modifierBits.size() - 1));

        events.clear();
        engine.tick(1_000_000);
        engine.release(1);
        assertTrue("a late release after closing sends nothing", events.isEmpty());
    }

    @Test
    public void closingWithNothingPressedSendsNothing() {
        engine.releaseAll();
        assertTrue(events.isEmpty());
    }

    @Test
    public void armedModifierUsedByAHeldKeyIsNotReleasedUnderIt() {
        engine.tapModifier(PcModifier.SHIFT);
        engine.press(key("A"), 1, 0);
        // The key is still down, so Shift tapped again arms for the next key without releasing early.
        engine.tapModifier(PcModifier.SHIFT);
        engine.release(1);
        engine.press(key("B"), 2, 0);
        engine.release(2);
        assertNothingStuck();
    }
}

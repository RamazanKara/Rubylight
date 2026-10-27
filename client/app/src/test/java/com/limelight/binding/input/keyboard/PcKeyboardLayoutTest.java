package com.limelight.binding.input.keyboard;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

import java.util.HashSet;
import java.util.List;
import java.util.Locale;
import java.util.Set;

public class PcKeyboardLayoutTest {
    private static final PcKeyboardLayout US = PcKeyboardLayout.byId(PcKeyboardLayout.US);
    private static final PcKeyboardLayout DE = PcKeyboardLayout.byId(PcKeyboardLayout.DE);

    private static PcKey byLabel(PcKeyboardLayout layout, String label) {
        PcKey found = null;
        for (List<PcKey> row : layout.rows()) {
            for (PcKey key : row) {
                if (key.label.equals(label)) {
                    assertNull("two keys read " + label, found);
                    found = key;
                }
            }
        }
        assertNotNull("no key reads " + label + " in " + layout.id, found);
        return found;
    }

    @Test
    public void everyLayoutHasTheFullPcKeySet() {
        for (PcKeyboardLayout layout : new PcKeyboardLayout[] {US, DE}) {
            String[] ids = {"Esc", "PrtSc", "ScrLk", "Pause", "Tab", "Caps", "Enter", "Backspace", "Space",
                    "Up", "Down", "Left", "Right", "Home", "End", "PgUp", "PgDn", "Ins", "Del",
                    "ShiftL", "ShiftR", "CtrlL", "CtrlR", "WinL", "WinR", "AltL", "Menu"};
            for (String id : ids) assertNotNull(layout.id + " lacks " + id, layout.key(id));
            for (int i = 1; i <= 12; i++) assertEquals(0x70 + i - 1, layout.key("F" + i).vk);
            for (char c = '0'; c <= '9'; c++) assertEquals(c, layout.key(String.valueOf(c)).vk);
            for (char c = 'A'; c <= 'Z'; c++) assertEquals(c, layout.key(String.valueOf(c)).vk);
        }
    }

    @Test
    public void keysAreUniqueAndStayInsideTheKeyboardWithoutOverlapping() {
        for (PcKeyboardLayout layout : new PcKeyboardLayout[] {US, DE}) {
            Set<String> ids = new HashSet<>();
            for (List<PcKey> row : layout.rows()) {
                float end = 0;
                for (PcKey key : row) {
                    assertTrue("duplicate id " + key.id, ids.add(key.id));
                    assertTrue(key.id + " overlaps its neighbour", key.x >= end - 0.001f);
                    end = key.x + key.width;
                }
                assertTrue(layout.id + " row is too wide", end <= layout.widthUnits() + 0.001f);
            }
        }
    }

    @Test
    public void bothLayoutsSendTheSameCodesFromTheSamePositions() {
        for (List<PcKey> row : US.rows()) {
            for (PcKey key : row) {
                PcKey other = DE.key(key.id);
                if (other == null) continue;
                assertEquals(key.id, key.vk, other.vk);
            }
        }
    }

    @Test
    public void germanZAndYSwapLegendsButNotPositions() {
        PcKey z = byLabel(DE, "Z"), y = byLabel(DE, "Y");
        assertEquals(0x59, z.vk);
        assertEquals(0x5A, y.vk);
        assertEquals(0x5A, byLabel(US, "Z").vk);
        assertEquals(0x59, byLabel(US, "Y").vk);
    }

    @Test
    public void germanUmlautsAndSharpSSitOnTheirIsoPositions() {
        assertEquals(0xDB, byLabel(DE, "Ü").vk);
        assertEquals(0xBA, byLabel(DE, "Ö").vk);
        assertEquals(0xDE, byLabel(DE, "Ä").vk);
        assertEquals(0xBD, byLabel(DE, "ß").vk);
        assertEquals(0xBB, byLabel(DE, "´").vk);
        assertEquals(0xC0, byLabel(DE, "^").vk);
        assertEquals(0xDD, byLabel(DE, "+").vk);
        assertEquals(0xDC, byLabel(DE, "#").vk);
        assertEquals(0xE2, byLabel(DE, "<").vk);
        assertEquals(0xBF, byLabel(DE, "-").vk);
        assertNull(US.key("IntlBackslash"));
    }

    @Test
    public void shiftAndAltGrLegendsFollowTheLayout() {
        assertEquals("@", US.key("2").labelFor(true, false));
        assertEquals("\"", DE.key("2").labelFor(true, false));
        assertEquals("²", DE.key("2").labelFor(false, true));
        assertEquals("2", DE.key("2").labelFor(false, false));
        assertEquals("@", DE.key("Q").labelFor(false, true));
        assertEquals("€", DE.key("E").labelFor(false, true));
        assertEquals("{", DE.key("7").labelFor(false, true));
        assertEquals("\\", DE.key("Minus").labelFor(false, true));
        assertEquals("?", DE.key("Minus").labelFor(true, false));
        assertEquals(":", US.key("Semicolon").labelFor(true, false));
        assertEquals("Q", US.key("Q").labelFor(true, false));
        // A key without an AltGr legend keeps its normal one.
        assertEquals("W", DE.key("W").labelFor(false, true));
    }

    @Test
    public void germanHasAltGrWhereUsHasRightAlt() {
        assertEquals(PcModifier.ALTGR, DE.key("AltGr").modifier);
        assertEquals(0xA5, DE.key("AltGr").vk);
        assertNull(DE.key("AltR"));
        assertEquals(PcModifier.ALT, US.key("AltR").modifier);
        assertEquals(0xA4, US.key("AltR").vk);
    }

    @Test
    public void layoutFollowsTheSystemLanguageUnlessChosen() {
        assertSame(DE, PcKeyboardLayout.resolve(PcKeyboardLayout.AUTO, Locale.GERMANY));
        assertSame(DE, PcKeyboardLayout.resolve(null, Locale.GERMAN));
        assertSame(US, PcKeyboardLayout.resolve(PcKeyboardLayout.AUTO, Locale.US));
        assertSame(US, PcKeyboardLayout.resolve(PcKeyboardLayout.AUTO, Locale.FRANCE));
        assertSame(US, PcKeyboardLayout.resolve(PcKeyboardLayout.US, Locale.GERMANY));
        assertSame(DE, PcKeyboardLayout.resolve(PcKeyboardLayout.DE, Locale.US));
        assertSame(US, PcKeyboardLayout.resolve("nonsense", Locale.US));
    }

    @Test
    public void germanLegendsOnlyWhereQwertzIsTheNorm() {
        for (String country : new String[] {"DE", "AT", "LU", "LI"}) {
            assertSame(country, DE, PcKeyboardLayout.resolve(PcKeyboardLayout.AUTO, new Locale("de", country)));
        }
        // Swiss keyboards and other German speakers get the US legends by default.
        assertSame(US, PcKeyboardLayout.resolve(PcKeyboardLayout.AUTO, new Locale("de", "CH")));
        assertSame(US, PcKeyboardLayout.resolve(PcKeyboardLayout.AUTO, new Locale("de", "BE")));
        assertSame(US, PcKeyboardLayout.resolve(PcKeyboardLayout.AUTO, new Locale("en", "DE")));
        assertSame("The user's choice still wins", DE, PcKeyboardLayout.resolve(PcKeyboardLayout.DE, new Locale("de", "CH")));
    }
}

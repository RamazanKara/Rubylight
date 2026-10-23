package com.limelight.binding.input.keyboard;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Locale;

/**
 * The keys of the on-screen PC keyboard, row by row. Every layout shares the physical key positions
 * and their codes, so a press always sends what a US keyboard sends from that spot; the layout
 * changes only the legends, so the key shows what a host set to the same layout types. German adds
 * the ISO key between Shift and Y, swaps Z and Y, and gives AltGr to the right Alt key.
 */
public final class PcKeyboardLayout {
    public static final String AUTO = "auto";
    public static final String US = "us";
    public static final String DE = "de";
    public static final String[] IDS = {US, DE};

    /** Width of the main block plus the navigation block, in key units. */
    private static final float WIDTH = 18.5f;
    private static final float NAV_X = 15.5f;

    private static final PcKeyboardLayout LAYOUT_US = build(US);
    private static final PcKeyboardLayout LAYOUT_DE = build(DE);

    public final String id;
    private final List<List<PcKey>> rows;

    private PcKeyboardLayout(String id, List<List<PcKey>> rows) {
        this.id = id;
        this.rows = rows;
    }

    public List<List<PcKey>> rows() {
        return rows;
    }

    public int rowCount() {
        return rows.size();
    }

    public float widthUnits() {
        return WIDTH;
    }

    /** The key with the given id, or null. Ids name the US key at that position, such as "Y" or "Backquote". */
    public PcKey key(String keyId) {
        for (List<PcKey> row : rows) {
            for (PcKey key : row) {
                if (key.id.equals(keyId)) return key;
            }
        }
        return null;
    }

    public static PcKeyboardLayout byId(String id) {
        return DE.equals(id) ? LAYOUT_DE : LAYOUT_US;
    }

    /** The layout for the preference value ("auto", "us" or "de"); auto follows the system language. */
    public static PcKeyboardLayout resolve(String preference, Locale locale) {
        if (DE.equals(preference)) return LAYOUT_DE;
        if (US.equals(preference)) return LAYOUT_US;
        return locale != null && "de".equals(locale.getLanguage()) ? LAYOUT_DE : LAYOUT_US;
    }

    private static PcKeyboardLayout build(String id) {
        boolean de = DE.equals(id);
        List<List<PcKey>> rows = new ArrayList<>();
        Row r = new Row(rows);

        // Function row.
        r.add("Esc", 0x1B, "Esc", 1, false);
        r.gap(1);
        for (int i = 0; i < 12; i++) {
            if (i == 4 || i == 8) r.gap(0.5f);
            r.add("F" + (i + 1), 0x70 + i, "F" + (i + 1), 1, true);
        }
        r.at(NAV_X);
        r.add("PrtSc", 0x2C, de ? "Druck" : "PrtSc", 1, false);
        r.add("ScrLk", 0x91, de ? "Rollen" : "ScrLk", 1, false);
        r.add("Pause", 0x13, "Pause", 1, false);

        // Number row.
        r = r.next();
        if (de) {
            r.add("Backquote", 0xC0, "^", "°", null, 1);
            r.add("1", 0x31, "1", "!", null, 1);
            r.add("2", 0x32, "2", "\"", "²", 1);
            r.add("3", 0x33, "3", "§", "³", 1);
            r.add("4", 0x34, "4", "$", null, 1);
            r.add("5", 0x35, "5", "%", null, 1);
            r.add("6", 0x36, "6", "&", null, 1);
            r.add("7", 0x37, "7", "/", "{", 1);
            r.add("8", 0x38, "8", "(", "[", 1);
            r.add("9", 0x39, "9", ")", "]", 1);
            r.add("0", 0x30, "0", "=", "}", 1);
            r.add("Minus", 0xBD, "ß", "?", "\\", 1);
            r.add("Equal", 0xBB, "´", "`", null, 1);
        } else {
            r.add("Backquote", 0xC0, "`", "~", null, 1);
            String shifted = "!@#$%^&*()";
            for (int i = 1; i <= 10; i++) {
                String digit = String.valueOf(i % 10);
                r.add(digit, 0x30 + i % 10, digit, String.valueOf(shifted.charAt(i - 1)), null, 1);
            }
            r.add("Minus", 0xBD, "-", "_", null, 1);
            r.add("Equal", 0xBB, "=", "+", null, 1);
        }
        r.add("Backspace", 0x08, "⌫", 2, true);
        r.at(NAV_X);
        r.add("Ins", 0x2D, de ? "Einfg" : "Ins", 1, true);
        r.add("Home", 0x24, de ? "Pos1" : "Home", 1, true);
        r.add("PgUp", 0x21, de ? "Bild↑" : "PgUp", 1, true);

        // Top letter row.
        r = r.next();
        r.add("Tab", 0x09, "Tab", 1.5f, true);
        String top = "QWERTYUIOP";
        for (int i = 0; i < top.length(); i++) {
            char c = top.charAt(i);
            // German swaps Z and Y: the key at the US Y position carries a Z.
            char legend = de && c == 'Y' ? 'Z' : c;
            String altGr = de && c == 'Q' ? "@" : de && c == 'E' ? "€" : null;
            r.add(String.valueOf(c), c, String.valueOf(legend), null, altGr, 1);
        }
        if (de) {
            r.add("BracketLeft", 0xDB, "Ü", null, null, 1);
            r.add("BracketRight", 0xDD, "+", "*", "~", 1);
            r.add("Enter", 0x0D, "↵", 1.5f, true);
        } else {
            r.add("BracketLeft", 0xDB, "[", "{", null, 1);
            r.add("BracketRight", 0xDD, "]", "}", null, 1);
            r.add("Backslash", 0xDC, "\\", "|", null, 1.5f);
        }
        r.at(NAV_X);
        r.add("Del", 0x2E, de ? "Entf" : "Del", 1, true);
        r.add("End", 0x23, de ? "Ende" : "End", 1, true);
        r.add("PgDn", 0x22, de ? "Bild↓" : "PgDn", 1, true);

        // Home row.
        r = r.next();
        r.add("Caps", 0x14, de ? "⇪" : "Caps", de ? 2.25f : 1.75f, false);
        String home = "ASDFGHJKL";
        for (int i = 0; i < home.length(); i++) {
            r.add(String.valueOf(home.charAt(i)), home.charAt(i), String.valueOf(home.charAt(i)), null, null, 1);
        }
        if (de) {
            r.add("Semicolon", 0xBA, "Ö", null, null, 1);
            r.add("Quote", 0xDE, "Ä", null, null, 1);
            r.add("Backslash", 0xDC, "#", "'", null, 1.75f);
        } else {
            r.add("Semicolon", 0xBA, ";", ":", null, 1);
            r.add("Quote", 0xDE, "'", "\"", null, 1);
            r.add("Enter", 0x0D, "↵", 2.25f, true);
        }

        // Bottom letter row.
        r = r.next();
        r.modifier("ShiftL", PcModifier.SHIFT, de ? "⇧" : "Shift", de ? 1.25f : 2.25f);
        if (de) r.add("IntlBackslash", 0xE2, "<", ">", "|", 1);
        String bottom = "ZXCVBNM";
        for (int i = 0; i < bottom.length(); i++) {
            char c = bottom.charAt(i);
            char legend = de && c == 'Z' ? 'Y' : c;
            r.add(String.valueOf(c), c, String.valueOf(legend), null, de && c == 'M' ? "µ" : null, 1);
        }
        if (de) {
            r.add("Comma", 0xBC, ",", ";", null, 1);
            r.add("Period", 0xBE, ".", ":", null, 1);
            r.add("Slash", 0xBF, "-", "_", null, 1);
        } else {
            r.add("Comma", 0xBC, ",", "<", null, 1);
            r.add("Period", 0xBE, ".", ">", null, 1);
            r.add("Slash", 0xBF, "/", "?", null, 1);
        }
        r.modifier("ShiftR", PcModifier.SHIFT, de ? "⇧" : "Shift", 2.75f);
        r.at(NAV_X + 1);
        r.add("Up", 0x26, "↑", 1, true);

        // Space row.
        r = r.next();
        String ctrl = de ? "Strg" : "Ctrl";
        r.modifier("CtrlL", PcModifier.CTRL, ctrl, 1.25f);
        r.modifier("WinL", PcModifier.WIN, "Win", 1.25f);
        r.modifier("AltL", PcModifier.ALT, "Alt", 1.25f);
        r.add("Space", 0x20, " ", 6.25f, true);
        if (de) r.modifier("AltGr", PcModifier.ALTGR, "AltGr", 1.25f);
        else r.modifier("AltR", PcModifier.ALT, "Alt", 1.25f);
        r.modifier("WinR", PcModifier.WIN, "Win", 1.25f);
        r.add("Menu", 0x5D, "Menu", 1.25f, false);
        r.modifier("CtrlR", PcModifier.CTRL, ctrl, 1.25f);
        r.at(NAV_X);
        r.add("Left", 0x25, "←", 1, true);
        r.add("Down", 0x28, "↓", 1, true);
        r.add("Right", 0x27, "→", 1, true);

        List<List<PcKey>> frozen = new ArrayList<>();
        for (List<PcKey> row : rows) frozen.add(Collections.unmodifiableList(row));
        return new PcKeyboardLayout(id, Collections.unmodifiableList(frozen));
    }

    /** Appends keys left to right to the current row. */
    private static final class Row {
        private final List<List<PcKey>> rows;
        private final List<PcKey> keys = new ArrayList<>();
        private float cursor;

        Row(List<List<PcKey>> rows) {
            this.rows = rows;
            rows.add(keys);
        }

        Row next() {
            return new Row(rows);
        }

        void gap(float width) {
            cursor += width;
        }

        void at(float x) {
            cursor = x;
        }

        void add(String id, int vk, String label, float width, boolean repeats) {
            keys.add(new PcKey(id, vk, label, null, null, null, cursor, width, repeats));
            cursor += width;
        }

        void add(String id, int vk, String label, String shiftLabel, String altGrLabel, float width) {
            keys.add(new PcKey(id, vk, label, shiftLabel, altGrLabel, null, cursor, width, true));
            cursor += width;
        }

        void modifier(String id, PcModifier modifier, String label, float width) {
            keys.add(new PcKey(id, modifier.vk, label, null, null, modifier, cursor, width, false));
            cursor += width;
        }
    }
}

package com.limelight.ui;

import android.text.InputType;
import android.view.inputmethod.EditorInfo;

import org.junit.Test;

import java.util.ArrayList;
import java.util.List;

import static com.limelight.ui.HostTextField.*;
import static org.junit.Assert.*;

public class HostTextFieldTest {
    private final List<String> sent = new ArrayList<>();
    private final HostTextField.Sink sink = edit -> sent.add(edit);
    // The host's field as the backspaces and text build it up.
    private final StringBuilder host = new StringBuilder();
    private final HostTextField.Sink typing = edit -> {
        for (int i = 0; i < edit.length(); ) {
            int c = edit.codePointAt(i);
            if (c == '\b') {
                if (host.length() > 0) host.setLength(host.offsetByCodePoints(host.length(), -1));
            } else {
                host.appendCodePoint(c);
            }
            i += Character.charCount(c);
        }
        return true;
    };

    @Test
    public void inputTypesFollowTheField() {
        assertEquals(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_PASSWORD |
                InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS, inputType(TYPE_PASSWORD, 0));
        assertEquals(InputType.TYPE_CLASS_NUMBER, inputType(TYPE_NUMBER, 0));
        assertEquals(InputType.TYPE_CLASS_NUMBER | InputType.TYPE_NUMBER_FLAG_DECIMAL, inputType(TYPE_DECIMAL, 0));
        assertEquals(InputType.TYPE_CLASS_PHONE, inputType(TYPE_PHONE, 0));
        assertEquals(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_AUTO_CORRECT |
                InputType.TYPE_TEXT_FLAG_CAP_SENTENCES, inputType(TYPE_TEXT, 0));
        assertEquals(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS,
                inputType(TYPE_TEXT, FLAG_NO_AUTOCORRECT | FLAG_NO_AUTOCAPITALIZE));
        assertEquals("a sensitive text field gets no suggestions",
                InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS,
                inputType(TYPE_TEXT, FLAG_SENSITIVE | FLAG_NO_AUTOCAPITALIZE));
        assertTrue((inputType(TYPE_MULTILINE, 0) & InputType.TYPE_TEXT_FLAG_MULTI_LINE) != 0);
        assertEquals(InputType.TYPE_TEXT_VARIATION_EMAIL_ADDRESS,
                inputType(TYPE_EMAIL, 0) & InputType.TYPE_MASK_VARIATION);
        assertEquals(InputType.TYPE_TEXT_VARIATION_URI, inputType(TYPE_URL, 0) & InputType.TYPE_MASK_VARIATION);
        assertEquals("an unknown type is text", inputType(TYPE_TEXT, 0), inputType(99, 0));
        for (int type = TYPE_TEXT; type <= TYPE_MULTILINE; type++) {
            assertNotEquals("never raw keys", InputType.TYPE_NULL, inputType(type, 0));
        }
    }

    @Test
    public void imeOptionsFollowTheField() {
        int noLearning = EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING;
        assertNotEquals(0, imeOptions(TYPE_PASSWORD, 0) & noLearning);
        assertNotEquals(0, imeOptions(TYPE_TEXT, FLAG_SENSITIVE) & noLearning);
        assertEquals(0, imeOptions(TYPE_TEXT, 0) & noLearning);
        assertEquals(EditorInfo.IME_ACTION_SEARCH, imeOptions(TYPE_SEARCH, 0) & EditorInfo.IME_MASK_ACTION);
        assertEquals(EditorInfo.IME_ACTION_GO, imeOptions(TYPE_TEXT, FLAG_ENTER_ACTION) & EditorInfo.IME_MASK_ACTION);
        assertEquals(EditorInfo.IME_ACTION_DONE, imeOptions(TYPE_TEXT, 0) & EditorInfo.IME_MASK_ACTION);
        assertNotEquals(0, imeOptions(TYPE_MULTILINE, FLAG_ENTER_ACTION) & EditorInfo.IME_FLAG_NO_ENTER_ACTION);
        for (int type = TYPE_TEXT; type <= TYPE_MULTILINE; type++) {
            int options = imeOptions(type, 0);
            assertNotEquals(0, options & EditorInfo.IME_FLAG_NO_FULLSCREEN);
            assertNotEquals(0, options & EditorInfo.IME_FLAG_NO_EXTRACT_UI);
        }
    }

    @Test
    public void composingIsForwardedAsItChanges() {
        HostTextField field = new HostTextField(TYPE_TEXT, 0);
        assertTrue(field.compose("h", sink));
        assertTrue(field.compose("he", sink));
        assertTrue(field.compose("hel", sink));
        assertTrue(field.compose("help", sink));
        assertTrue(field.commit("Hello ", sink));
        assertEquals(List.of("h", "e", "l", "p", "\b\b\b\bHello "), sent);
        assertEquals("Hello ", field.textBeforeCursor(100));
        assertEquals(6, field.cursor());
        assertEquals(-1, field.composingStart());
    }

    @Test
    public void theHostEndsUpWithWhatTheKeyboardShows() {
        HostTextField field = new HostTextField(TYPE_TEXT, 0);
        field.compose("Gru", typing);
        field.compose("Grü", typing);
        field.compose("Grüße", typing);
        field.commit("Grüße", typing);
        field.compose("😀", typing);
        field.compose("😃", typing);
        field.commit("😃 ", typing);
        field.compose("x", typing);
        field.compose("", typing);
        field.finishComposing();
        assertEquals("Grüße😃 ", host.toString());
        assertEquals(host.toString(), field.textBeforeCursor(100));
        assertTrue("three UTF-16 units: the emoji and the space", field.deleteBefore(3, typing));
        assertEquals("Grüße", host.toString());
        assertEquals("Grüße", field.textBeforeCursor(100));
    }

    @Test
    public void surrogatePairsAreOneBackspace() {
        HostTextField field = new HostTextField(TYPE_TEXT, 0);
        field.compose("a😀", sink);
        field.compose("a😃", sink);
        assertEquals("\b😃", sent.get(1));
        field.finishComposing();
        field.deleteBefore(1, sink);
        assertEquals("a low surrogate alone is the whole emoji", "\b", sent.get(2));
        assertEquals("a", field.textBeforeCursor(10));
    }

    @Test
    public void deletingPastTheTypedTextStillReachesTheHost() {
        HostTextField field = new HostTextField(TYPE_TEXT, 0);
        field.commit("ab", sink);
        assertTrue(field.deleteBefore(5, sink));
        assertEquals("text the field held before the raise is deleted too", "\b\b\b\b\b", sent.get(1));
        assertTrue(field.deleteBefore(0, sink));
        assertEquals(2, sent.size());
    }

    @Test
    public void aFailedSendKeepsTheMirrorOfTheHost() {
        HostTextField field = new HostTextField(TYPE_TEXT, 0);
        field.compose("ab", typing);
        assertFalse(field.compose("abc", edit -> false));
        assertTrue(field.compose("abd", typing));
        assertEquals("the next change is diffed against what the host has", "abd", host.toString());
        field.finishComposing();
        assertFalse(field.deleteBefore(1, edit -> false));
        assertEquals("abd", field.textBeforeCursor(10));
    }

    @Test
    public void onlyTextEndingAtTheCursorRecomposes() {
        HostTextField field = new HostTextField(TYPE_TEXT, 0);
        field.commit("hello world", typing);
        assertFalse(field.recompose(0, 5));
        assertTrue(field.recompose(6, 11));
        assertEquals(6, field.composingStart());
        assertEquals(11, field.composingEnd());
        field.compose("word", typing);
        field.commit("word", typing);
        assertEquals("hello word", host.toString());
        assertFalse(field.recompose(-1, 10));
    }

    @Test
    public void contextIsBoundedButPositionsKeepCounting() {
        HostTextField field = new HostTextField(TYPE_TEXT, 0);
        StringBuilder all = new StringBuilder();
        for (int i = 0; i < MAX_CONTEXT; i++) {
            field.commit("ab", typing);
            all.append("ab");
        }
        assertEquals(all.length(), field.cursor());
        assertEquals(MAX_CONTEXT, field.textBeforeCursor(Integer.MAX_VALUE).length());
        assertTrue(field.recompose(field.cursor() - 2, field.cursor()));
        field.commit("cd", typing);
        assertEquals(all.substring(0, all.length() - 2) + "cd", host.toString());
    }

    @Test
    public void aRaiseNeverShowsOverAHardwareKeyboardOrTheMenus() {
        assertTrue(mayShowKeyboard(true, true, false, true, false, false));
        assertFalse("hardware keyboard", mayShowKeyboard(true, true, false, true, false, true));
        assertFalse("stream menu", mayShowKeyboard(true, true, true, true, false, false));
        assertFalse("PC keyboard", mayShowKeyboard(true, true, false, true, true, false));
        assertFalse("view only", mayShowKeyboard(true, true, false, false, false, false));
        assertFalse("background", mayShowKeyboard(false, true, false, true, false, false));
        assertFalse("not connected", mayShowKeyboard(true, false, false, true, false, false));
    }
}

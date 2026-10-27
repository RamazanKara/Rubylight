package com.limelight.ui;

import android.text.InputType;
import android.view.inputmethod.EditorInfo;

/**
 * The host's focused text field (0x553B notices) as the soft keyboard sees it: its input type and
 * IME options, and the text typed into it since it was raised, which only the host holds. The IME
 * composes words before it commits them; every change goes to the host at once as backspaces and
 * new text ("\b" per removed code point, then the added text), so the host's field shows what the
 * keyboard shows and nothing is typed twice. The text the field held before is unknown here.
 */
public final class HostTextField {
    public static final int KIND_LOWER = 0, KIND_RAISE = 1, KIND_UPDATE = 2;
    public static final int TYPE_TEXT = 0, TYPE_NUMBER = 1, TYPE_DECIMAL = 2, TYPE_PHONE = 3,
            TYPE_EMAIL = 4, TYPE_URL = 5, TYPE_PASSWORD = 6, TYPE_SEARCH = 7, TYPE_MULTILINE = 8;
    public static final int FLAG_SENSITIVE = 1, FLAG_NO_AUTOCORRECT = 2, FLAG_NO_AUTOCAPITALIZE = 4,
            FLAG_ENTER_ACTION = 8;
    // Typed text kept for the IME's capitalization and suggestions; older text is dropped.
    static final int MAX_CONTEXT = 256;

    /** Sends text to the host; '\b' is a backspace. False when it could not be sent. */
    public interface Sink {
        boolean send(String edit);
    }

    public final int type, flags;
    private final StringBuilder committed = new StringBuilder();
    private String composing = "";
    // Characters dropped from the front of committed, so positions stay those the IME was told.
    private int dropped;

    public HostTextField(int type, int flags) {
        this.type = type;
        this.flags = flags;
    }

    /**
     * Whether a raise may show the soft keyboard: not over the stream menu, the on-screen PC
     * keyboard, a view-only stream or an attached hardware keyboard, which types for itself.
     */
    public static boolean mayShowKeyboard(boolean foreground, boolean connected, boolean menuOpen,
                                          boolean canSendInput, boolean pcKeyboardVisible,
                                          boolean hardwareKeyboard) {
        return foreground && connected && !menuOpen && canSendInput && !pcKeyboardVisible && !hardwareKeyboard;
    }

    public int inputType() {
        return inputType(type, flags);
    }

    public int imeOptions() {
        return imeOptions(type, flags);
    }

    public static int inputType(int type, int flags) {
        boolean sensitive = (flags & FLAG_SENSITIVE) != 0;
        boolean noAutocorrect = sensitive || (flags & FLAG_NO_AUTOCORRECT) != 0;
        int text = InputType.TYPE_CLASS_TEXT | (noAutocorrect ? InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
                : InputType.TYPE_TEXT_FLAG_AUTO_CORRECT);
        if ((flags & FLAG_NO_AUTOCAPITALIZE) == 0) text |= InputType.TYPE_TEXT_FLAG_CAP_SENTENCES;
        switch (type) {
            case TYPE_NUMBER:
                return InputType.TYPE_CLASS_NUMBER;
            case TYPE_DECIMAL:
                return InputType.TYPE_CLASS_NUMBER | InputType.TYPE_NUMBER_FLAG_DECIMAL;
            case TYPE_PHONE:
                return InputType.TYPE_CLASS_PHONE;
            case TYPE_EMAIL:
                return InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_EMAIL_ADDRESS |
                        (noAutocorrect ? InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS : 0);
            case TYPE_URL:
                return InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_URI |
                        (noAutocorrect ? InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS : 0);
            case TYPE_PASSWORD:
                // No suggestions: nothing typed into a password field is learned or shown.
                return InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_PASSWORD |
                        InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS;
            case TYPE_MULTILINE:
                return text | InputType.TYPE_TEXT_FLAG_MULTI_LINE;
            default:
                return text;
        }
    }

    @android.annotation.SuppressLint("InlinedApi") // A flag older IMEs ignore.
    public static int imeOptions(int type, int flags) {
        int options = EditorInfo.IME_FLAG_NO_EXTRACT_UI | EditorInfo.IME_FLAG_NO_FULLSCREEN;
        if ((flags & FLAG_SENSITIVE) != 0 || type == TYPE_PASSWORD) {
            options |= EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING;
        }
        if (type == TYPE_SEARCH) {
            options |= EditorInfo.IME_ACTION_SEARCH;
        } else if (type == TYPE_MULTILINE) {
            // Enter types a new line.
            options |= EditorInfo.IME_ACTION_NONE | EditorInfo.IME_FLAG_NO_ENTER_ACTION;
        } else if ((flags & FLAG_ENTER_ACTION) != 0) {
            options |= EditorInfo.IME_ACTION_GO;
        } else {
            options |= EditorInfo.IME_ACTION_DONE;
        }
        return options;
    }

    /** setComposingText: the composing text becomes text. */
    public boolean compose(CharSequence text, Sink sink) {
        return replaceComposing(text == null ? "" : text.toString(), sink);
    }

    /** commitText: text replaces the composing text and is final. */
    public boolean commit(CharSequence text, Sink sink) {
        boolean sent = replaceComposing(text == null ? "" : text.toString(), sink);
        finishComposing();
        return sent;
    }

    /** finishComposingText: the composing text, already on the host, is final. */
    public void finishComposing() {
        committed.append(composing);
        composing = "";
        int excess = committed.length() - MAX_CONTEXT;
        if (excess > 0) {
            if (Character.isLowSurrogate(committed.charAt(excess))) excess++;
            committed.delete(0, excess);
            dropped += excess;
        }
    }

    /**
     * deleteSurroundingText: removes up to before characters before the cursor and sends a
     * backspace for each code point; text after the cursor is never here, the caller sends those.
     */
    public boolean deleteBefore(int before, Sink sink) {
        finishComposing();
        int count = Math.min(Math.max(before, 0), committed.length());
        int start = committed.length() - count;
        if (start > 0 && start < committed.length() && Character.isLowSurrogate(committed.charAt(start))) {
            start--;
        }
        int codePoints = committed.codePointCount(start, committed.length());
        // Text from before the field was raised has no mirror here; it still goes.
        codePoints += Math.max(before, 0) - count;
        if (codePoints == 0) return true;
        if (!sink.send(backspaces(codePoints))) return false;
        committed.setLength(start);
        return true;
    }

    /**
     * setComposingRegion: only the text that ends at the cursor can be composed again, as the host's
     * caret is always at the end of what was typed. False otherwise, leaving everything as it was.
     */
    public boolean recompose(int start, int end) {
        finishComposing();
        int from = Math.min(start, end) - dropped, to = Math.max(start, end) - dropped;
        if (to != committed.length() || from < 0 || from > to) return false;
        composing = committed.substring(from);
        committed.setLength(from);
        return true;
    }

    /** The cursor position as the IME counts it. */
    public int cursor() {
        return dropped + committed.length() + composing.length();
    }

    public int composingStart() {
        return composing.isEmpty() ? -1 : dropped + committed.length();
    }

    public int composingEnd() {
        return composing.isEmpty() ? -1 : cursor();
    }

    public String textBeforeCursor(int length) {
        String all = committed + composing;
        int start = Math.max(0, all.length() - Math.max(length, 0));
        if (start > 0 && Character.isLowSurrogate(all.charAt(start))) start++;
        return all.substring(start);
    }

    private boolean replaceComposing(String text, Sink sink) {
        String old = composing;
        int prefix = 0, max = Math.min(old.length(), text.length());
        while (prefix < max && old.charAt(prefix) == text.charAt(prefix)) prefix++;
        if (prefix > 0 && Character.isHighSurrogate(old.charAt(prefix - 1))) prefix--;
        String edit = backspaces(old.codePointCount(prefix, old.length())) + text.substring(prefix);
        if (edit.isEmpty()) {
            composing = text;
            return true;
        }
        boolean sent = sink.send(edit);
        // Nothing went when the send failed, so the host still shows the old composing text.
        if (sent) composing = text;
        return sent;
    }

    private static String backspaces(int count) {
        StringBuilder edit = new StringBuilder(count);
        for (int i = 0; i < count; i++) edit.append('\b');
        return edit.toString();
    }
}

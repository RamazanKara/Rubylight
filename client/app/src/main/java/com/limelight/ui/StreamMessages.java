package com.limelight.ui;

import android.content.Context;
import android.os.SystemClock;
import android.widget.Toast;

import com.limelight.computers.HostNotices;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * What a stream tells the person, one message at a time. Something to act on, or that failed,
 * replaces whatever shows. A lasting "not supported" fact goes to the overlay's status line, and
 * shows as a message once per PC, and only while nothing else shows; otherwise a later stream says it.
 */
public final class StreamMessages {
    // How long Android keeps a toast up.
    static final long LONG_MS = 3500;
    static final long SHORT_MS = 2000;

    private final Context context;
    private final Set<String> notes = new LinkedHashSet<>();
    private final Map<String, String> statuses = new LinkedHashMap<>();
    private Toast current;
    private CharSequence currentText;
    private long visibleUntil;

    public StreamMessages(Context context) {
        this.context = context;
    }

    /** Something to act on, or a failure: shown now in place of what shows, unless it already shows. */
    public void alert(CharSequence text, int duration) {
        if (text == null || text.length() == 0) return;
        if (showing() && text.toString().contentEquals(currentText)) return;
        show(text, duration);
    }

    public void alert(int text, int duration) {
        alert(context.getString(text), duration);
    }

    /**
     * A fact that stays true for this PC. It joins the status line, and shows once per PC when nothing
     * else shows; while something does, the PC is not marked, so a later stream says it instead.
     */
    public void notice(String pcUuid, HostNotices.Notice notice, CharSequence text) {
        note(text);
        if (!showing() && HostNotices.claim(context, pcUuid, notice)) {
            show(text, Toast.LENGTH_LONG);
        }
    }

    /** A fact the connection already counted once for this PC: it shows only when nothing else does. */
    public void countedNotice(CharSequence text) {
        note(text);
        if (!showing()) show(text, Toast.LENGTH_LONG);
    }

    /** A fact for the overlay's status line alone. */
    public void note(CharSequence text) {
        if (text != null && text.length() != 0) notes.add(text.toString());
    }

    /**
     * A fact for the overlay's status line that can stop being true, such as a screen that did not
     * follow the frame rate vote. One line per key; null or empty text removes it. Returns whether
     * the line changed.
     */
    public boolean status(String key, CharSequence text) {
        if (text == null || text.length() == 0) return statuses.remove(key) != null;
        return !text.toString().equals(statuses.put(key, text.toString()));
    }

    public List<String> notes() {
        List<String> all = new ArrayList<>(notes);
        all.addAll(statuses.values());
        return all;
    }

    public boolean showing() {
        return current != null && SystemClock.uptimeMillis() < visibleUntil;
    }

    private void show(CharSequence text, int duration) {
        if (current != null) current.cancel();
        current = Toast.makeText(context, text, duration);
        currentText = text;
        visibleUntil = SystemClock.uptimeMillis() + (duration == Toast.LENGTH_LONG ? LONG_MS : SHORT_MS);
        current.show();
    }
}

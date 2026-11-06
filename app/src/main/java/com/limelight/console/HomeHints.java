package com.limelight.console;

import android.content.Context;
import android.view.View;
import android.widget.TextView;

import com.limelight.R;
import com.limelight.ui.InputSource;
import com.limelight.ui.KeyCaps;

/**
 * The button hints of Home on a phone and a handheld: a key cap and a word for Go and More on what has focus, for the device
 * in the hand. They change on the next input with no animation, and are gone for touch and a mouse.
 */
final class HomeHints {
    private final Context context;
    private final TextView view;
    private InputSource source;
    private int vendor;
    private CharSequence go;
    private CharSequence more;

    /** {@code view} may be null: a layout without the floating line. */
    HomeHints(Context context, TextView view, InputSource start) {
        this.context = context;
        this.view = view;
        this.source = start;
        write();
    }

    /** The next input came from {@code next} (null says nothing) on a device of {@code nextVendor}. */
    void onInput(InputSource next, int nextVendor) {
        if (next == null || (next == source && nextVendor == vendor)) return;
        source = next;
        vendor = nextVendor;
        write();
    }

    /** What Go and More do on what has focus now; a null word has no hint, two nulls no line. */
    void onFocus(CharSequence goWord, CharSequence moreWord) {
        go = goWord;
        more = moreWord;
        write();
    }

    InputSource source() {
        return source;
    }

    int vendor() {
        return vendor;
    }

    private void write() {
        if (view == null) return;
        CharSequence text = line(context, source, vendor, go, more);
        boolean shown = text != null;
        if (shown) view.setText(text);
        view.setVisibility(shown ? View.VISIBLE : View.GONE);
    }

    /** The line with its keys drawn as key caps; null when there is nothing to say on this source. */
    static CharSequence line(Context context, InputSource source, int vendor, CharSequence go, CharSequence more) {
        String goKey = source == null ? null : source.goKey(context, vendor);
        String moreKey = source == null ? null : source.moreKey(context, vendor);
        if (goKey == null || (go == null && more == null)) return null;
        if (more == null) return KeyCaps.mark(context, goKey + " " + go, goKey);
        if (go == null) return KeyCaps.mark(context, moreKey + " " + more, moreKey);
        return KeyCaps.mark(context, context.getString(R.string.console_hint_line, goKey, go, moreKey, more), goKey, moreKey);
    }
}

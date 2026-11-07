package com.limelight.ui;

import android.text.Layout;
import android.text.StaticLayout;
import android.text.TextPaint;
import android.util.TypedValue;
import android.widget.TextView;

/**
 * Lays the long performance overlay out on a screen where nothing can scroll it: a TV's remote goes
 * to the PC during a stream. Keeps its text size in one column if it can, then in two, and only
 * then goes smaller. Pure measuring: the caller sets the result on its views.
 */
public final class OverlayFit {
    /** Still readable from a sofa: the text never goes below this. */
    static final float SMALLEST_SP = 9;

    public final CharSequence first;
    /** Null while one column holds it all. */
    public final CharSequence second;
    public final float textSp;
    /** The widest a column may be. */
    public final int columnWidthPx;

    private OverlayFit(CharSequence first, CharSequence second, float textSp, int columnWidthPx) {
        this.first = first;
        this.second = second;
        this.textSp = textSp;
        this.columnWidthPx = columnWidthPx;
    }

    /**
     * @param view       a column, for its font and padding
     * @param widthPx    room for the columns, without the overlay's margins and padding
     * @param heightPx   room for a column, likewise
     * @param maxColumnPx the widest a column is on any screen
     */
    public static OverlayFit fit(TextView view, CharSequence text, float textSp, int widthPx, int heightPx, int maxColumnPx) {
        int one = Math.min(maxColumnPx, widthPx);
        int two = Math.min(maxColumnPx, widthPx / 2);
        int room = heightPx - view.getTotalPaddingTop() - view.getTotalPaddingBottom();
        OverlayFit smallest = null;
        for (float sp = textSp; sp >= SMALLEST_SP; sp--) {
            if (layout(view, text, sp, one).getHeight() <= room) return new OverlayFit(text, null, sp, one);
            Layout columns = layout(view, text, sp, two);
            int split = splitAt(text, columns, room);
            CharSequence first = trimEnd(text.subSequence(0, split));
            CharSequence second = trimStart(text.subSequence(split, text.length()));
            smallest = new OverlayFit(first, second, sp, two);
            if (layout(view, second, sp, two).getHeight() <= room) return smallest;
        }
        // More than two columns of the smallest text: the first lines stay, the rest runs off.
        return smallest != null ? smallest : new OverlayFit(text, null, textSp, one);
    }

    private static StaticLayout layout(TextView view, CharSequence text, float sp, int widthPx) {
        TextPaint paint = new TextPaint(view.getPaint());
        paint.setTextSize(TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_SP, sp, view.getResources().getDisplayMetrics()));
        int inner = Math.max(1, widthPx - view.getTotalPaddingLeft() - view.getTotalPaddingRight());
        return StaticLayout.Builder.obtain(text, 0, text.length(), paint, inner)
                .setLineSpacing(view.getLineSpacingExtra(), view.getLineSpacingMultiplier())
                .setIncludePad(view.getIncludeFontPadding())
                .build();
    }

    /** Where the first column ends: the last whole line that fits, or a section's gap just before it. */
    private static int splitAt(CharSequence text, Layout columns, int room) {
        int line = 0;
        while (line + 1 < columns.getLineCount() && columns.getLineBottom(line + 1) <= room) line++;
        for (int gap = line; gap > line * 2 / 3; gap--) {
            int start = columns.getLineStart(gap);
            if (columns.getLineEnd(gap) - start <= 1 && text.charAt(start) == '\n') return columns.getLineStart(gap + 1);
        }
        return columns.getLineEnd(line);
    }

    private static CharSequence trimEnd(CharSequence text) {
        int end = text.length();
        while (end > 0 && text.charAt(end - 1) == '\n') end--;
        return text.subSequence(0, end);
    }

    private static CharSequence trimStart(CharSequence text) {
        int start = 0;
        while (start < text.length() && text.charAt(start) == '\n') start++;
        return text.subSequence(start, text.length());
    }
}

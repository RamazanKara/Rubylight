package com.limelight.ui;

import android.graphics.Canvas;
import android.graphics.Paint;
import android.os.Build;
import android.text.Layout;
import android.text.SpannableStringBuilder;
import android.text.Spanned;
import android.text.StaticLayout;
import android.text.TextPaint;
import android.text.style.RelativeSizeSpan;
import android.text.style.ReplacementSpan;
import android.util.TypedValue;
import android.view.View;
import android.widget.TextView;

import com.limelight.R;

/**
 * Lays the long performance overlay out on a screen where nothing can scroll it: a TV's remote goes to the PC during a
 * stream. One column along the screen's edge is the layout: as it is when it fits, then compact (low gaps between the
 * sections, a short pacing graph), then in smaller text down to a size still read from a sofa. A second column, which
 * covers more of the game, is the last resort for what even that cannot hold. Pure measuring: {@link #show} sets it.
 */
public final class OverlayFit {
    /** Still readable from a sofa: the text never goes below this. */
    static final float SMALLEST_SP = 9;
    /** A section's gap in a compact overlay, in lines. */
    static final float COMPACT_GAP = 0.4f;
    /** The pacing graph's height in a compact overlay, in lines. */
    static final float COMPACT_GRAPH = 0.6f;
    /** The pacing timeline's levels, as {@code PresentJitter} writes them: a dot, then one to eight vsyncs. */
    private static final String LEVELS = ".▁▂▃▄▅▆▇█";

    public final CharSequence first;
    /** Null while one column holds it all. */
    public final CharSequence second;
    public final float textSp;
    /** The widest a column may be. */
    public final int columnWidthPx;
    /** Whether the gaps and the graph were made smaller to fit. */
    public final boolean compact;

    private OverlayFit(CharSequence first, CharSequence second, float textSp, int columnWidthPx, boolean compact) {
        this.first = first;
        this.second = second;
        this.textSp = textSp;
        this.columnWidthPx = columnWidthPx;
        this.compact = compact;
    }

    /**
     * @param view       a column, for its font and padding
     * @param widthPx    room for the columns, without the overlay's margins and padding
     * @param heightPx   room for a column, likewise
     * @param maxColumnPx the widest a column is on any screen
     */
    public static OverlayFit fit(TextView view, CharSequence text, float textSp, int widthPx, int heightPx, int maxColumnPx) {
        int one = Math.min(maxColumnPx, widthPx);
        int room = heightPx - view.getTotalPaddingTop() - view.getTotalPaddingBottom();
        if (layout(view, text, textSp, one).getHeight() <= room) return new OverlayFit(text, null, textSp, one, false);
        CharSequence tight = compact(text);
        for (float sp = textSp; sp >= SMALLEST_SP; sp--) {
            if (layout(view, tight, sp, one).getHeight() <= room) return new OverlayFit(tight, null, sp, one, true);
        }
        // Even the smallest column runs off the screen: two, at the largest size both hold.
        int two = Math.min(maxColumnPx, widthPx / 2);
        OverlayFit smallest = null;
        for (float sp = textSp; sp >= SMALLEST_SP; sp--) {
            Layout columns = layout(view, tight, sp, two);
            int split = splitAt(tight, columns, room);
            CharSequence first = trimEnd(tight.subSequence(0, split));
            CharSequence second = trimStart(tight.subSequence(split, tight.length()));
            smallest = new OverlayFit(first, second, sp, two, true);
            if (layout(view, second, sp, two).getHeight() <= room) return smallest;
        }
        // More than two columns of the smallest text: the first lines stay, the rest runs off.
        return smallest != null ? smallest : new OverlayFit(tight, null, textSp, one, true);
    }

    /**
     * Shows {@code text} in the overlay's two columns. Where a finger can scroll it, or a short overlay that always fits,
     * it is the first column at {@code textSp}; otherwise it is fitted to the room its parent leaves.
     */
    public static void show(TextView first, TextView second, View scroll, CharSequence text, float textSp, boolean mayBeLong,
                            boolean canScroll) {
        View parent = scroll != null ? (View) scroll.getParent() : null;
        int margin = Math.round(16 * first.getResources().getDisplayMetrics().density);
        int width = parent != null ? parent.getWidth() - margin - scroll.getPaddingLeft() - scroll.getPaddingRight() : 0;
        int height = parent != null ? parent.getHeight() - margin - scroll.getPaddingTop() - scroll.getPaddingBottom() : 0;
        int column = first.getResources().getDimensionPixelSize(R.dimen.stream_overlay_column);
        // Not laid out yet: the next second's update fits it.
        if (!mayBeLong || width <= 0 || height <= 0 || canScroll) {
            first.setTextSize(textSp);
            first.setText(text);
            second.setVisibility(View.GONE);
            first.setMaxWidth(column);
            return;
        }
        OverlayFit fit = fit(first, text, textSp, width, height, column);
        first.setTextSize(fit.textSp);
        first.setMaxWidth(fit.columnWidthPx);
        first.setText(fit.first);
        second.setTextSize(fit.textSp);
        second.setMaxWidth(fit.columnWidthPx);
        second.setText(fit.second);
        second.setVisibility(fit.second != null ? View.VISIBLE : View.GONE);
    }

    /** The overlay with low gaps between its sections and its pacing timeline drawn as a short bar graph. */
    public static CharSequence compact(CharSequence text) {
        SpannableStringBuilder out = new SpannableStringBuilder(text);
        int start = 0;
        for (int i = 0; i <= out.length(); i++) {
            if (i < out.length() && out.charAt(i) != '\n') continue;
            if (i == start && i < out.length() && start > 0) {
                // An empty line: the gap before a section.
                out.setSpan(new RelativeSizeSpan(COMPACT_GAP), i, i + 1, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
            } else if (isGraph(out, start, i)) {
                // The line's break shrinks with it, or the line keeps the text's height.
                out.setSpan(new RelativeSizeSpan(COMPACT_GRAPH), start, Math.min(i + 1, out.length()), Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
                out.setSpan(new PacingGraph(), start, i, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
            }
            start = i + 1;
        }
        return out;
    }

    private static boolean isGraph(CharSequence text, int start, int end) {
        if (end - start < 8) return false;
        boolean bar = false;
        for (int i = start; i < end; i++) {
            int level = LEVELS.indexOf(text.charAt(i));
            if (level < 0) return false;
            if (level > 0) bar = true;
        }
        return bar;
    }

    /**
     * The pacing timeline as bars in the text's colour, one per column of a digit's width, standing on the baseline. The
     * glyphs come from a fallback font whose taller lines spread the overlay; bars keep to the line they are given.
     */
    static final class PacingGraph extends ReplacementSpan {
        @Override
        public int getSize(Paint paint, CharSequence text, int start, int end, Paint.FontMetricsInt fm) {
            if (fm != null) paint.getFontMetricsInt(fm);
            return Math.round(paint.measureText("0") * (end - start));
        }

        @Override
        public void draw(Canvas canvas, CharSequence text, int start, int end, float x, int top, int y, int bottom, Paint paint) {
            Paint.FontMetrics metrics = paint.getFontMetrics();
            float column = paint.measureText("0");
            float bar = Math.max(1f, column * 0.66f);
            float full = (metrics.descent - metrics.ascent) * 0.85f;
            float floor = y + metrics.descent * 0.5f;
            for (int i = start; i < end; i++) {
                int level = Math.max(0, LEVELS.indexOf(text.charAt(i)));
                float height = level == 0 ? Math.max(1f, full / 16f) : full * level / 8f;
                float left = x + (i - start) * column + (column - bar) / 2f;
                canvas.drawRect(left, floor - height, left + bar, floor, paint);
            }
        }
    }

    private static StaticLayout layout(TextView view, CharSequence text, float sp, int widthPx) {
        TextPaint paint = new TextPaint(view.getPaint());
        paint.setTextSize(TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_SP, sp, view.getResources().getDisplayMetrics()));
        int inner = Math.max(1, widthPx - view.getTotalPaddingLeft() - view.getTotalPaddingRight());
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M) {
            // Before Android 6 a TextView has no break strategy or hyphenation to copy: simple breaking, as here.
            return new StaticLayout(text, paint, inner, Layout.Alignment.ALIGN_NORMAL,
                    view.getLineSpacingMultiplier(), view.getLineSpacingExtra(), view.getIncludeFontPadding());
        }
        StaticLayout.Builder builder = StaticLayout.Builder.obtain(text, 0, text.length(), paint, inner)
                .setLineSpacing(view.getLineSpacingExtra(), view.getLineSpacingMultiplier())
                .setIncludePad(view.getIncludeFontPadding())
                .setBreakStrategy(view.getBreakStrategy())
                .setHyphenationFrequency(view.getHyphenationFrequency());
        // A TextView from Android 9 makes room for a fallback font's taller lines: measure as it draws.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) builder.setUseLineSpacingFromFallbacks(view.isFallbackLineSpacing());
        return builder.build();
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

package com.limelight.ui;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.RectF;
import android.text.SpannableString;
import android.text.Spanned;
import android.text.style.ReplacementSpan;

/**
 * Key names in a hint ("X or ⋮ for options", "A Open") drawn as keycaps: each key in a rounded outline, so it reads as
 * a button and not as a stray letter. The hint stays one translated string; only its keys are marked.
 */
public final class KeyCaps {
    private KeyCaps() { }

    /**
     * {@code text} with each of {@code keys} drawn as a keycap: the first whole-word match of each key, searched in
     * order after the previous one. A key that is not in the text is skipped.
     */
    public static CharSequence mark(Context context, CharSequence text, String... keys) {
        SpannableString marked = new SpannableString(text);
        float density = context.getResources().getDisplayMetrics().density;
        String plain = text.toString();
        int from = 0;
        for (String key : keys) {
            int at = find(plain, key, from);
            if (at < 0) continue;
            marked.setSpan(new KeyCapSpan(density), at, at + key.length(), Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
            from = at + key.length();
        }
        return marked;
    }

    /** Where {@code key} starts as a whole word at or after {@code from}, or -1. */
    static int find(String text, String key, int from) {
        if (key == null || key.isEmpty()) return -1;
        for (int at = text.indexOf(key, from); at >= 0; at = text.indexOf(key, at + 1)) {
            int end = at + key.length();
            boolean startsWord = at == 0 || !Character.isLetterOrDigit(text.charAt(at - 1));
            boolean endsWord = end == text.length() || !Character.isLetterOrDigit(text.charAt(end));
            if (startsWord && endsWord) return at;
        }
        return -1;
    }

    /** A key's text inside a rounded outline in the text's own colour, as tall as the line's glyphs. */
    static final class KeyCapSpan extends ReplacementSpan {
        private final float padding;
        private final float stroke;
        private final float radius;
        private final RectF rect = new RectF();

        KeyCapSpan(float density) {
            padding = 6 * density;
            stroke = Math.max(1f, density);
            radius = 4 * density;
        }

        @Override
        public int getSize(Paint paint, CharSequence text, int start, int end, Paint.FontMetricsInt fm) {
            if (fm != null) paint.getFontMetricsInt(fm);
            return Math.round(paint.measureText(text, start, end) + padding * 2);
        }

        @Override
        public void draw(Canvas canvas, CharSequence text, int start, int end, float x, int top, int y, int bottom,
                         Paint paint) {
            Paint.FontMetrics metrics = paint.getFontMetrics();
            float width = paint.measureText(text, start, end) + padding * 2;
            float inset = stroke / 2;
            rect.set(x + inset, y + metrics.ascent + inset, x + width - inset, y + metrics.descent - inset);
            Paint.Style style = paint.getStyle();
            float strokeWidth = paint.getStrokeWidth();
            int alpha = paint.getAlpha();
            paint.setStyle(Paint.Style.STROKE);
            paint.setStrokeWidth(stroke);
            paint.setAlpha(alpha * 3 / 4);
            canvas.drawRoundRect(rect, radius, radius, paint);
            paint.setStyle(style);
            paint.setStrokeWidth(strokeWidth);
            paint.setAlpha(alpha);
            boolean bold = paint.isFakeBoldText();
            paint.setFakeBoldText(true);
            canvas.drawText(text, start, end, x + padding, y, paint);
            paint.setFakeBoldText(bold);
        }
    }
}

package com.limelight.ui;

import android.app.Application;
import android.graphics.Typeface;
import android.text.SpannableString;
import android.text.Spanned;
import android.text.StaticLayout;
import android.text.style.RelativeSizeSpan;
import android.text.style.StyleSpan;
import android.widget.TextView;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = 29, application = Application.class, qualifiers = "w960dp-h540dp-xhdpi")
public class OverlayFitTest {
    private static final int WIDTH = 1920, COLUMN = 880;
    private static final String GRAPH = "▁▁▂▁▁▁█▁▁▁.▁▁▁▁▁";

    private static TextView view() {
        return new TextView(RuntimeEnvironment.getApplication());
    }

    private static String lines(int count) {
        StringBuilder text = new StringBuilder();
        for (int i = 0; i < count; i++) {
            if (i > 0) text.append(i % 7 == 0 ? "\n\n" : "\n");
            text.append(i == 3 ? GRAPH : "Line " + i);
        }
        return text.toString();
    }

    private static int height(TextView view, CharSequence text, float sp) {
        view.setTextSize(sp);
        return StaticLayout.Builder.obtain(text, 0, text.length(), view.getPaint(), COLUMN)
                .setUseLineSpacingFromFallbacks(view.isFallbackLineSpacing()).build().getHeight();
    }

    private static void assertNothingLost(CharSequence all, OverlayFit fit) {
        String joined = (fit.first + (fit.second != null ? "\n" + fit.second : "")).replace("\n", "");
        assertEquals(all.toString().replace("\n", ""), joined);
    }

    @Test public void whatFitsStaysOneColumnAtItsSizeAndLooks() {
        String text = lines(10);
        OverlayFit fit = OverlayFit.fit(view(), text, 12, WIDTH, 1000, COLUMN);
        assertEquals(text, fit.first.toString());
        assertNull(fit.second);
        assertFalse(fit.compact);
        assertEquals(12, fit.textSp, 0);
        assertEquals(COLUMN, fit.columnWidthPx);
    }

    @Test public void compactLowersTheGapsAndTheGraphOnly() {
        TextView view = view();
        String text = lines(30);
        CharSequence compact = OverlayFit.compact(text);
        assertEquals("the same characters", text, compact.toString());
        Spanned spans = (Spanned) compact;
        int gaps = 0;
        for (RelativeSizeSpan span : spans.getSpans(0, compact.length(), RelativeSizeSpan.class)) {
            if (span.getSizeChange() == OverlayFit.COMPACT_GAP) {
                gaps++;
                assertEquals("on a gap's empty line", "\n", compact.subSequence(spans.getSpanStart(span), spans.getSpanEnd(span)).toString());
            }
        }
        assertEquals(4, gaps);
        OverlayFit.PacingGraph[] graph = spans.getSpans(0, compact.length(), OverlayFit.PacingGraph.class);
        assertEquals(1, graph.length);
        assertEquals(GRAPH, compact.subSequence(spans.getSpanStart(graph[0]), spans.getSpanEnd(graph[0])).toString());
        assertTrue(height(view, compact, 12) < height(view, text, 12));
        // A row of dots alone, or short text, is no graph.
        assertEquals(0, ((Spanned) OverlayFit.compact("a\n........\n▁▁")).getSpans(0, 13, OverlayFit.PacingGraph.class).length);
    }

    @Test public void tooLongForOneColumnGoesCompactBeforeItShrinks() {
        TextView view = view();
        String text = lines(40);
        int room = (height(view, text, 12) + height(view, OverlayFit.compact(text), 12)) / 2;
        OverlayFit fit = OverlayFit.fit(view(), text, 12, WIDTH, room, COLUMN);
        assertNull(fit.second);
        assertTrue(fit.compact);
        assertEquals(12, fit.textSp, 0);
        assertTrue(height(view, fit.first, 12) <= room);
    }

    @Test public void tooLongWhenCompactShrinksInOneColumnBeforeASecond() {
        TextView view = view();
        String text = lines(40);
        int room = height(view, text, 12) * 3 / 4;
        OverlayFit fit = OverlayFit.fit(view(), text, 12, WIDTH, room, COLUMN);
        assertNull("one column while the smallest text holds it", fit.second);
        assertTrue(fit.textSp < 12 && fit.textSp >= OverlayFit.SMALLEST_SP);
        assertEquals(COLUMN, fit.columnWidthPx);
        assertTrue(height(view, fit.first, fit.textSp) <= room);
    }

    @Test public void onlyWhatTheSmallestColumnCannotHoldTakesTwo() {
        TextView view = view();
        String text = lines(70);
        int room = height(view, OverlayFit.compact(text), OverlayFit.SMALLEST_SP) * 3 / 4;
        OverlayFit fit = OverlayFit.fit(view(), text, 12, WIDTH, room, COLUMN);
        assertNotNull(fit.second);
        assertTrue(fit.compact);
        assertNothingLost(text, fit);
        assertTrue(height(view, fit.first, fit.textSp) <= room);
        assertTrue(height(view, fit.second, fit.textSp) <= room);
        // Two columns hold more: they take the largest size both fit, not the smallest.
        assertTrue(fit.textSp > OverlayFit.SMALLEST_SP);
    }

    @Test public void aScreenTooSmallForEvenThatStopsAtTheSmallestSize() {
        OverlayFit fit = OverlayFit.fit(view(), lines(400), 12, WIDTH, 300, COLUMN);
        assertEquals(OverlayFit.SMALLEST_SP, fit.textSp, 0);
        assertNotNull(fit.second);
    }

    @Test public void aNarrowScreenSharesItsWidth() {
        OverlayFit fit = OverlayFit.fit(view(), lines(400), 12, 1000, 300, COLUMN);
        assertEquals(500, fit.columnWidthPx);
    }

    @Test public void theColumnsBreakAtASectionsGapAndKeepTheirStyles() {
        SpannableString text = new SpannableString(lines(70));
        text.setSpan(new StyleSpan(Typeface.BOLD), 0, 4, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
        TextView view = view();
        int room = height(view, OverlayFit.compact(text), OverlayFit.SMALLEST_SP) * 3 / 4;
        OverlayFit fit = OverlayFit.fit(view(), text, 12, WIDTH, room, COLUMN);
        assertTrue(fit.first instanceof Spanned);
        assertEquals(1, ((Spanned) fit.first).getSpans(0, 4, StyleSpan.class).length);
        int firstLine = Integer.parseInt(fit.second.toString().split("\n")[0].substring("Line ".length()));
        assertEquals("starts a section", 0, firstLine % 7);
    }
}

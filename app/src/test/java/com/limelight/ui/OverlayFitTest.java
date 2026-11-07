package com.limelight.ui;

import android.app.Application;
import android.graphics.Typeface;
import android.text.SpannableString;
import android.text.Spanned;
import android.text.StaticLayout;
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

    private static TextView view() {
        return new TextView(RuntimeEnvironment.getApplication());
    }

    private static String lines(int count) {
        StringBuilder text = new StringBuilder();
        for (int i = 0; i < count; i++) {
            if (i > 0) text.append(i % 7 == 0 ? "\n\n" : "\n");
            text.append("Line ").append(i);
        }
        return text.toString();
    }

    private static int height(TextView view, CharSequence text, float sp) {
        view.setTextSize(sp);
        return StaticLayout.Builder.obtain(text, 0, text.length(), view.getPaint(), COLUMN).build().getHeight();
    }

    private static void assertNothingLost(CharSequence all, OverlayFit fit) {
        String joined = (fit.first + "\n" + fit.second).replace("\n", "");
        assertEquals(all.toString().replace("\n", ""), joined);
    }

    @Test public void whatFitsStaysOneColumnAtItsSize() {
        String text = lines(10);
        OverlayFit fit = OverlayFit.fit(view(), text, 12, WIDTH, 1000, COLUMN);
        assertEquals(text, fit.first.toString());
        assertNull(fit.second);
        assertEquals(12, fit.textSp, 0);
        assertEquals(COLUMN, fit.columnWidthPx);
    }

    @Test public void tooLongForOneColumnGoesToTwoBeforeItShrinks() {
        TextView view = view();
        String text = lines(40);
        int room = height(view, text, 12) * 2 / 3;
        OverlayFit fit = OverlayFit.fit(view(), text, 12, WIDTH, room, COLUMN);
        assertNotNull(fit.second);
        assertEquals(12, fit.textSp, 0);
        assertNothingLost(text, fit);
        assertTrue(height(view, fit.first, 12) <= room);
        assertTrue(height(view, fit.second, 12) <= room);
    }

    @Test public void tooLongForTwoColumnsShrinksUntilItFits() {
        TextView view = view();
        String text = lines(70);
        int room = height(view, text, 12) * 2 / 5;
        OverlayFit fit = OverlayFit.fit(view(), text, 12, WIDTH, room, COLUMN);
        assertTrue(fit.textSp < 12 && fit.textSp >= OverlayFit.SMALLEST_SP);
        assertNothingLost(text, fit);
        assertTrue(height(view, fit.second, fit.textSp) <= room);
    }

    @Test public void aScreenTooSmallForEvenThatStopsAtTheSmallestSize() {
        OverlayFit fit = OverlayFit.fit(view(), lines(400), 12, WIDTH, 300, COLUMN);
        assertEquals(OverlayFit.SMALLEST_SP, fit.textSp, 0);
        assertNotNull(fit.second);
    }

    @Test public void aNarrowScreenSharesItsWidth() {
        OverlayFit fit = OverlayFit.fit(view(), lines(40), 12, 1000, 300, COLUMN);
        assertEquals(500, fit.columnWidthPx);
    }

    @Test public void theColumnsBreakAtASectionsGapAndKeepTheirStyles() {
        SpannableString text = new SpannableString(lines(40));
        text.setSpan(new StyleSpan(Typeface.BOLD), 0, 4, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
        TextView view = view();
        int room = height(view, text, 12) * 2 / 3;
        OverlayFit fit = OverlayFit.fit(view(), text, 12, WIDTH, room, COLUMN);
        assertTrue(fit.first instanceof Spanned);
        assertEquals(1, ((Spanned) fit.first).getSpans(0, 4, StyleSpan.class).length);
        int firstLine = Integer.parseInt(fit.second.toString().split("\n")[0].substring("Line ".length()));
        assertEquals("starts a section", 0, firstLine % 7);
    }
}

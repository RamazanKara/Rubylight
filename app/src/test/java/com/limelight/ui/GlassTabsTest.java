package com.limelight.ui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Color;
import android.view.ContextThemeWrapper;
import android.view.View;
import android.widget.LinearLayout;

import com.limelight.R;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;

/** The current tab's pill and the focus ring: one strong mark at a time (nightjar#7). */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = 28, application = Application.class)
public class GlassTabsTest {
    @After
    public void forget() {
        GlassTabs.clearLast();
    }

    private static GlassTabs tabs() {
        GlassTabs tabs = new GlassTabs(new ContextThemeWrapper(RuntimeEnvironment.getApplication(), R.style.AppTheme));
        for (int i = 0; i < 3; i++) {
            View tab = new View(tabs.getContext());
            tab.setFocusable(true);
            tab.setFocusableInTouchMode(true);
            tab.setSelected(i == 0);
            tabs.addView(tab, new LinearLayout.LayoutParams(120, 48));
        }
        tabs.measure(View.MeasureSpec.makeMeasureSpec(360, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(48, View.MeasureSpec.EXACTLY));
        tabs.layout(0, 0, 360, 48);
        return tabs;
    }

    /** The pill's opacity in the middle of the first tab, away from the ring at its edge. */
    private static int pillAlphaAtFirstTab(GlassTabs tabs) {
        Bitmap bitmap = Bitmap.createBitmap(360, 48, Bitmap.Config.ARGB_8888);
        tabs.draw(new Canvas(bitmap));
        return Color.alpha(bitmap.getPixel(60, 24));
    }

    @Test
    public void anotherFocusedTabLeavesOnlyTheRingStrong() {
        GlassTabs tabs = tabs();
        int alone = pillAlphaAtFirstTab(tabs);
        assertTrue("the theme gives the pill a fill", alone > 0);
        assertFalse(tabs.pillBehindFocus());

        // Focus on the current tab: pill and ring together mark one tab.
        assertTrue(tabs.getChildAt(0).requestFocus());
        assertFalse(tabs.pillBehindFocus());
        assertEquals(alone, pillAlphaAtFirstTab(tabs));

        // Focus on another tab: the pill steps back, so only the focused tab reads as chosen.
        assertTrue(tabs.getChildAt(1).requestFocus());
        assertTrue(tabs.pillBehindFocus());
        int behind = pillAlphaAtFirstTab(tabs);
        assertTrue("pill " + behind + " of " + alone, behind > 0 && behind <= Math.round(alone * GlassTabs.PILL_BEHIND_FOCUS) + 1);

        // Focus leaving the bar gives the pill back in full.
        tabs.getChildAt(1).clearFocus();
        assertFalse(tabs.pillBehindFocus());
        assertEquals(alone, pillAlphaAtFirstTab(tabs));
    }
}

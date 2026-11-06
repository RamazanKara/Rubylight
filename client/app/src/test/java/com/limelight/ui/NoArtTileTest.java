package com.limelight.ui;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertTrue;

import android.content.Context;
import android.graphics.Bitmap;
import android.view.View;

import androidx.appcompat.view.ContextThemeWrapper;
import androidx.core.content.ContextCompat;
import org.robolectric.RuntimeEnvironment;

import com.limelight.R;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

/** Android and the desktop client give a title the same look: these values come from the design prototype's own hash. */
@RunWith(RobolectricTestRunner.class)
@org.robolectric.annotation.GraphicsMode(org.robolectric.annotation.GraphicsMode.Mode.NATIVE)
@Config(sdk = 34, application = android.app.Application.class)
public class NoArtTileTest {
    private static void pinned(String title, int hash, int pair, int disc) {
        assertEquals(title, hash, NoArtTile.hash(title));
        assertEquals(title, pair, NoArtTile.pairOf(title));
        assertEquals(title, disc, NoArtTile.discOf(title));
    }

    @Test
    public void theHashPicksWhatThePrototypeAndTheDesktopClientPick() {
        pinned("", 0x811c9dc5, 1, 1);
        pinned("a", 0xe40c292c, 4, 1);
        pinned("Desktop", 0xdd8087b3, 3, 3);
        pinned("Big Picture", 0x988584b3, 11, 0);
        pinned("Photo Frame", 0x5eca700a, 10, 0);
        pinned("Lunar Base", 0x72263bf0, 4, 3);
        pinned("Karaoke Night", 0xdb5e012f, 7, 1);
        pinned("Game Launcher", 0x6ef16931, 1, 1);
        pinned("Media Center", 0xf6243618, 0, 2);
        pinned("Studio PC", 0x92b89bf8, 4, 3);
        pinned("Game-room PC", 0x44a970ca, 2, 0);
        pinned("日本語", 0x805f5ce7, 3, 0);
    }

    @Test
    public void capitalsDoNotChangeTheLook() {
        assertEquals(NoArtTile.hash("Lunar Base"), NoArtTile.hash("LUNAR BASE"));
    }

    @Test
    public void theDiscSitsWhereTheSpecPutsIt() {
        assertArrayEquals(new float[]{.74f, .62f}, NoArtTile.discCentre("Big Picture"), 0f);
        assertArrayEquals(new float[]{.26f, .7f}, NoArtTile.discCentre("Karaoke Night"), 0f);
        assertArrayEquals(new float[]{.8f, .42f}, NoArtTile.discCentre("Media Center"), 0f);
        assertArrayEquals(new float[]{.5f, .78f}, NoArtTile.discCentre("Lunar Base"), 0f);
    }

    private static Context themed() {
        return new ContextThemeWrapper(RuntimeEnvironment.getApplication(), R.style.AppTheme);
    }

    private static NoArtTile laidOut(String title, int width) {
        NoArtTile tile = new NoArtTile(themed());
        tile.setText(title);
        tile.measure(View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(width * 3 / 2, View.MeasureSpec.EXACTLY));
        tile.layout(0, 0, width, width * 3 / 2);
        return tile;
    }

    @Test
    public void theTitleTakesItsSizeAndInsetFromTheWidth() {
        NoArtTile tile = laidOut("Lunar Base", 300);
        assertEquals(30, tile.getPaddingLeft());
        assertEquals(30, tile.getPaddingTop());
        assertEquals(300 * .16f, tile.titleSize(), .5f);
        assertEquals(3, tile.getMaxLines());
    }

    @Test
    public void aWordTooLongToFitDropsOnceToTheSmallerSize() {
        NoArtTile tile = laidOut("Supercalifragilisticexpialidocious", 300);
        assertEquals(300 * .125f, tile.titleSize(), .5f);
    }

    @Test
    public void theLongestWordIsTheOneThatMustFit() {
        assertEquals("Extraordinary", NoArtTile.longestWord("  An Extraordinary Day "));
    }

    @Test
    public void theAmbientIsTheTilesOwnGradientOnePerPair() {
        Context context = themed();
        Bitmap ambient = NoArtTile.ambientSource(context, "Lunar Base");
        assertSame(ambient, NoArtTile.ambientSource(context, "Studio PC"));
        assertNotEquals(ambient, NoArtTile.ambientSource(context, "Big Picture"));
        // Pair 4 from the top-left corner: close to the token, and not the page's colour.
        int topLeft = ambient.getPixel(0, 0);
        int token = ContextCompat.getColor(context, R.color.nj_tile_pair4_top);
        assertTrue(Math.abs(android.graphics.Color.red(topLeft) - android.graphics.Color.red(token)) < 40);
    }
}

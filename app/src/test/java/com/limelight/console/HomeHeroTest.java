package com.limelight.console;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.widget.FrameLayout;

import org.robolectric.RuntimeEnvironment;

import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.ui.InputSource;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;

/** Home's hero, its hints, its search and the chrome around them. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class HomeHeroTest {
    private final Context context = RuntimeEnvironment.getApplication();

    private static HomeModel.GameCard card(String title, HomeModel.Badge badge, Long lastPlayed) {
        return new HomeModel.GameCard("k:" + title, title, "tower", "Tower", title.hashCode(), badge, null, 1, false, false,
                Collections.singletonList("tower"), lastPlayed);
    }

    @Test
    public void playSaysResumeForARunningGame() {
        assertEquals(R.string.library_play, HomeHero.playLabel(card("A", HomeModel.Badge.NONE, null)));
        assertEquals(R.string.applist_menu_resume, HomeHero.playLabel(card("A", HomeModel.Badge.RUNNING, null)));
    }

    @Test
    public void aGameThatNewlyLeadsTakesTheHeroAndAPersonsChoiceOtherwiseStays() {
        HomeModel.GameCard racing = card("Racing", HomeModel.Badge.RUNNING, null);
        HomeModel.GameCard world = card("World", HomeModel.Badge.NONE, 1L);
        HomeModel.GameCard ocean = card("Ocean", HomeModel.Badge.NONE, 2L);
        HomeHero stored = new HomeHero(Arrays.asList(world, ocean), HomeHero.Source.CONTINUE);
        assertEquals("the first show", world.key, stored.shownAfterRefresh(null, null));

        // The first read of the PC finds a running game: it comes to the hero though World was showing.
        HomeHero read = new HomeHero(Arrays.asList(racing, world, ocean), HomeHero.Source.CONTINUE);
        assertEquals(racing.key, read.shownAfterRefresh(world.key, world.key));

        // The person moved to Ocean: the next refresh with the same lead leaves it there.
        assertEquals(ocean.key, read.shownAfterRefresh(ocean.key, racing.key));
    }

    @Test
    public void aPcThatDoesNotAnswerOpensTheGamesPageInsteadOfStarting() {
        assertTrue("no such PC", HomeHero.opensPage(card("A", HomeModel.Badge.NONE, null), null));
        ComputerDetails asleep = new ComputerDetails();
        asleep.state = ComputerDetails.State.OFFLINE;
        assertTrue(HomeHero.opensPage(card("A", HomeModel.Badge.NONE, null), asleep));
    }

    @Test
    public void theCoverFollowsTheWindowAndFitsTheHeight() {
        assertEquals(290, HomeHero.coverWidth(412, 915));
        assertTrue("a short window narrows the cover", HomeHero.coverWidth(412, 500) < 290);
    }

    @Test
    public void stepsFromTheMiddleCountPitches() {
        assertEquals(0f, HomeHero.stepsFromMiddle(500, 500, 100), 0.001f);
        assertEquals(1f, Math.abs(HomeHero.stepsFromMiddle(600, 500, 100)), 0.001f);
    }

    /** Android's default highlight fills the button's bounds: square corners behind the capsule when it has focus. */
    @Test
    public void aGlassButtonHasOnlyItsRingForFocus() {
        Context themed = new android.view.ContextThemeWrapper(context, R.style.AppTheme);
        assertFalse(new com.limelight.ui.GlassButton(themed, null).getDefaultFocusHighlightEnabled());
    }

    /** A panel line pressed on a handheld swaps with the hero's game: neither shows twice or goes missing. */
    @Test
    public void thePanelHoldsTheHerosOtherGames() {
        HomeModel.GameCard a = card("A", HomeModel.Badge.NONE, null);
        HomeModel.GameCard b = card("B", HomeModel.Badge.NONE, null);
        HomeModel.GameCard c = card("C", HomeModel.Badge.NONE, null);
        HomeHero hero = new HomeHero(Arrays.asList(a, b, c), HomeHero.Source.CONTINUE);
        assertEquals(Arrays.asList(b, c), ConsoleHomeAdapter.panelCards(hero, a));
        assertEquals(Arrays.asList(a, c), ConsoleHomeAdapter.panelCards(hero, b));
        assertTrue(ConsoleHomeAdapter.panelCards(new HomeHero(Arrays.asList(a, b), HomeHero.Source.LIBRARY), a).isEmpty());
    }

    /** A last-played time from the host is in seconds: read as milliseconds every game was played in January 1970. */
    @Test
    public void aPanelLineReadsTheHostsSeconds() {
        long now = 1_761_300_000_000L;
        String line = HomeHeroViews.lineOf(context, card("A", HomeModel.Badge.NONE, now / 1000 - 40 * 86_400), now).toString();
        assertFalse(line, line.contains("1970"));
        assertTrue(line, line.contains("2025"));
    }

    @Test
    public void hintsFollowTheDeviceInTheHand() {
        assertEquals(InputSource.PAD, InputSource.of(new KeyEvent(0, 0, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_A, 0, 0, 0, 0, 0,
                InputDevice.SOURCE_GAMEPAD)));
        assertEquals(InputSource.PAD, InputSource.of(new KeyEvent(0, 0, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_DOWN, 0, 0, 0, 0, 0,
                InputDevice.SOURCE_JOYSTICK)));
        assertEquals("a D-pad that is no pad is a remote's", InputSource.REMOTE, InputSource.of(new KeyEvent(0, 0, KeyEvent.ACTION_DOWN,
                KeyEvent.KEYCODE_DPAD_CENTER, 0, 0, 0, 0, 0, InputDevice.SOURCE_DPAD)));
        assertEquals("the volume keys say nothing about the device", null,
                InputSource.of(new KeyEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_VOLUME_UP)));
    }

    /** The key that ends touch mode is eaten before the window sees it: the first focus must still have its hint. */
    @Test
    public void theFirstKeyAfterAFingerStillHasHints() {
        assertEquals(InputSource.PAD, InputSource.leavingTouch(true, true));
        assertEquals(InputSource.KEYBOARD, InputSource.leavingTouch(false, true));
        assertEquals(InputSource.REMOTE, InputSource.leavingTouch(false, false));
    }

    @Test
    public void theFloatingHintsLeaveForTouchAndComeBackForKeys() {
        android.widget.TextView view = new android.widget.TextView(context);
        HomeHints hints = new HomeHints(context, view, InputSource.NONE);
        hints.onFocus("Open", "Options");
        assertEquals(android.view.View.GONE, view.getVisibility());
        hints.onInput(InputSource.PAD, 0);
        assertEquals(android.view.View.VISIBLE, view.getVisibility());
        assertTrue(view.getText().toString(), view.getText().toString().startsWith("A "));
        hints.onInput(InputSource.NONE, 0);
        assertEquals(android.view.View.GONE, view.getVisibility());
        hints.onInput(InputSource.PAD, 0);
        hints.onFocus(null, null);
        assertEquals("chrome in focus has nothing to say", android.view.View.GONE, view.getVisibility());
    }

    private static List<String> titles(List<HomeModel.GameCard> cards) {
        List<String> titles = new ArrayList<>();
        for (HomeModel.GameCard card : cards) titles.add(card.title);
        return titles;
    }

    @Test
    public void searchFindsEveryWordIgnoringCaseAndAccents() {
        List<HomeModel.GameCard> games = Arrays.asList(card("Space Sim", HomeModel.Badge.NONE, null),
                card("Pokémon Night", HomeModel.Badge.NONE, null), card("Sim City", HomeModel.Badge.NONE, null),
                card("Night Sim", HomeModel.Badge.NONE, null));
        assertEquals(Arrays.asList("Pokémon Night"), titles(HomeSearch.matches(games, "POKEMON")));
        assertEquals(Arrays.asList("Night Sim", "Pokémon Night"), titles(HomeSearch.matches(games, "night")));
        assertEquals(Arrays.asList("Night Sim"), titles(HomeSearch.matches(games, "sim night")));
        assertEquals(Arrays.asList("Sim City", "Night Sim", "Space Sim"), titles(HomeSearch.matches(games, "sim")));
        assertEquals("an empty search has them all, by title", 4, HomeSearch.matches(games, "  ").size());
        assertTrue(HomeSearch.matches(games, "zelda").isEmpty());
    }

    /** The spec's stops, not a straight ramp: the shelf's words under the bar must not read through it. */
    @Test
    public void theFootFadeUsesTheDesignStops() {
        assertArrayEquals(new float[] {0f, 0.42f, 0.72f, 1f}, HomeChrome.FOOT_STOPS, 0f);
        int[] colors = HomeChrome.footColors(0xFF101820);
        assertEquals(0, colors[0] >>> 24);
        assertEquals(158, colors[1] >>> 24);
        assertEquals(255, colors[3] >>> 24);
    }

    /** A focused tab's ring is drawn past the tab: the strip must not cut it unless the tabs have to scroll. */
    @Test
    public void theTabStripClipsOnlyWhenTheTabsScroll() {
        FrameLayout strip = new FrameLayout(context);
        strip.layout(0, 0, 500, 80);
        HomeChrome.fitRing(strip, 400);
        assertFalse(strip.getClipChildren());
        HomeChrome.fitRing(strip, 600);
        assertTrue(strip.getClipChildren());
    }
}

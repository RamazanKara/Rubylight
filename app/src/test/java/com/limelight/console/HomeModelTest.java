package com.limelight.console;

import static com.limelight.nativeapi.NativeApiFixtures.classicHost;
import static com.limelight.nativeapi.NativeApiFixtures.game;
import static com.limelight.nativeapi.NativeApiFixtures.library;
import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static com.limelight.nativeapi.NativeApiFixtures.session;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;

import com.limelight.nativeapi.ConsoleSnapshot;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;

import org.junit.After;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;
import java.util.Set;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class HomeModelTest {
    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    private ConsoleRig rig;

    @Before
    public void setUp() throws Exception {
        rig = new ConsoleRig(temporary.newFolder());
    }

    @After
    public void tearDown() {
        rig.close();
    }

    private static Set<String> online(String... uuids) {
        return new HashSet<>(Arrays.asList(uuids));
    }

    private void tower() {
        rig.transport.reply("tower", "library", ok(library("t1",
                game("t-portal", 11, "Portal", "steam:400", false), game("t-zelda", 12, "Zelda", null, false))));
        rig.transport.reply("tower", "session", ok(NativeApiFixtures.IDLE));
    }

    private void laptop() {
        rig.transport.reply("laptop", "library", ok(library("l1",
                game("l-portal", 5, "portal", "steam:400", false), game("l-alan", 6, "Alan Wake", null, true))));
        rig.transport.reply("laptop", "session", ok(session("streaming", "other_device",
                "{\"install_id\":\"l-alan\",\"app_id\":6,\"title\":\"Alan Wake\"}", "[]")));
    }

    private static List<String> titles(List<HomeModel.GameCard> cards) {
        List<String> titles = new ArrayList<>();
        for (HomeModel.GameCard card : cards) {
            titles.add(card.title);
        }
        return titles;
    }

    @Test
    public void continuePlayingKeepsTheRepositoryOrderAndMapsSessionsToBadges() throws Exception {
        rig.transport.reply("tower", "library", ok(library("t", game("t-portal", 11, "Portal", "steam:400", false))));
        rig.transport.reply("tower", "session", ok(session("suspended", "you",
                "{\"install_id\":\"t-portal\",\"app_id\":11,\"title\":\"Portal\"}", "[{\"id\":\"resume\",\"allowed\":true}]")));
        laptop();
        List<ComputerDetails> hosts = Arrays.asList(nativeHost("tower", "Tower", true), nativeHost("laptop", "Laptop", true));
        HomeModel model = HomeModel.of(rig.repository.refresh(hosts), online("tower", "laptop"));
        assertEquals(Arrays.asList("Alan Wake", "Portal"), titles(model.continuePlaying));
        assertEquals(HomeModel.Badge.RUNNING, model.continuePlaying.get(0).badge);
        assertEquals("Laptop", model.continuePlaying.get(0).hostName);
        assertEquals(HomeModel.Badge.SUSPENDED, model.continuePlaying.get(1).badge);
    }

    @Test
    public void libraryMergesTheSameGameOnTwoHostsAndCountsThem() throws Exception {
        tower();
        laptop();
        List<ComputerDetails> hosts = Arrays.asList(nativeHost("tower", "Tower", true), nativeHost("laptop", "Laptop", true));
        HomeModel model = HomeModel.of(rig.repository.refresh(hosts), online("tower", "laptop"));
        HomeModel.GameCard portal = null;
        for (HomeModel.GameCard card : model.library) {
            if (card.title.equalsIgnoreCase("portal")) {
                portal = card;
            }
        }
        assertNotNull(portal);
        assertEquals(2, portal.hostCount);
        assertEquals(3, model.library.size());
        for (HomeModel.GameCard card : model.library) {
            if (card.title.equals("Alan Wake")) {
                assertEquals(HomeModel.Badge.RUNNING, card.badge);
                assertEquals(1, card.hostCount);
            }
        }
    }

    @Test
    public void favouritesAreTheFirstRowKeyedByHostAndGameSoTheSameTitleOnTwoPcsIsTwoEntries() throws Exception {
        tower();
        laptop();
        rig.repository.setFavourite("laptop", "Laptop", 5, "portal", true);
        rig.repository.setFavourite("tower", "Tower", 11, "Portal", true);
        rig.repository.setFavourite("tower", "Tower", 12, "Zelda", true);
        rig.repository.setFavourite("tower", "Tower", 12, "Zelda", false);
        List<ComputerDetails> hosts = Arrays.asList(nativeHost("tower", "Tower", true), nativeHost("laptop", "Laptop", true));
        HomeModel model = HomeModel.of(rig.repository.refresh(hosts), online("tower", "laptop"));
        // Newest star first; each entry names its host.
        assertEquals(Arrays.asList("Portal", "portal"), titles(model.favourites));
        assertEquals("Tower", model.favourites.get(0).hostName);
        assertEquals("Laptop", model.favourites.get(1).hostName);
        assertTrue(model.favourites.get(0).favourite);
        assertFalse(model.favourites.get(0).key.equals(model.favourites.get(1).key));
        // The merged library card is starred for the host it opens (the first one).
        for (HomeModel.GameCard card : model.library) {
            assertEquals(card.title.equalsIgnoreCase("portal"), card.favourite);
        }
    }

    @Test
    public void aFavouriteTheHostNoLongerListsIsHiddenButKeptAndAnOfflineHostKeepsItsFavourites() throws Exception {
        tower();
        rig.repository.setFavourite("tower", "Tower", 12, "Zelda", true);
        rig.repository.setFavourite("tower", "Tower", 99, "Uninstalled", true);
        ComputerDetails live = nativeHost("tower", "Tower", true);
        HomeModel model = HomeModel.of(rig.repository.refresh(Collections.singletonList(live)), online("tower"));
        assertEquals(Collections.singletonList("Zelda"), titles(model.favourites));
        assertTrue(rig.repository.isFavourite("tower", 99));

        // Offline: the saved library is all there is, and the favourites come from it.
        HomeModel saved = HomeModel.of(rig.repository.cached(
                Collections.singletonList(nativeHost("tower", "Tower", false))), online());
        assertEquals(Collections.singletonList("Zelda"), titles(saved.favourites));
    }

    @Test
    public void aFavouriteOfAHostWithoutALibraryYetIsListedByItsStoredName() throws Exception {
        rig.transport.unreachable("tower");
        rig.repository.setFavourite("tower", "Tower", 11, "Portal", true);
        HomeModel model = HomeModel.of(rig.repository.refresh(
                Collections.singletonList(nativeHost("tower", "Tower", true))), online("tower"));
        assertEquals(Collections.singletonList("Portal"), titles(model.favourites));
        assertNull(model.favourites.get(0).source);
    }

    @Test
    public void aFavouriteOfAHostThatWasForgottenIsGone() throws Exception {
        tower();
        rig.repository.setFavourite("tower", "Tower", 11, "Portal", true);
        rig.repository.forgetHost("tower");
        assertFalse(rig.repository.isFavourite("tower", 11));
    }

    @Test
    public void aGameThatIsNotInstalledIsNotInTheLibrary() throws Exception {
        rig.transport.reply("tower", "library", ok(library("t1",
                game("t-portal", 11, "Portal", "steam:400", false),
                game("t-gone", 12, "Gone", null, false).replace("\"installed\":true", "\"installed\":false"))));
        rig.transport.reply("tower", "session", ok(NativeApiFixtures.IDLE));
        HomeModel model = HomeModel.of(rig.repository.refresh(
                Collections.singletonList(nativeHost("tower", "Tower", true))), online("tower"));
        assertEquals(Collections.singletonList("Portal"), titles(model.library));
    }

    @Test
    public void hostCardsSayWhatTheHostShows() throws Exception {
        tower();
        ComputerDetails live = nativeHost("tower", "Tower", true);
        HomeModel fresh = HomeModel.of(rig.repository.refresh(Collections.singletonList(live)), online("tower"));
        assertEquals(1, fresh.hosts.size());
        assertEquals(HomeModel.HostLine.GAMES, fresh.hosts.get(0).line);
        assertEquals(2, fresh.hosts.get(0).gameCount);
        assertTrue(fresh.hosts.get(0).online);

        // The host went away: the saved library is still listed, labelled as saved.
        ComputerDetails away = nativeHost("tower", "Tower", false);
        HomeModel saved = HomeModel.of(rig.repository.cached(Collections.singletonList(away)), online());
        assertEquals(HomeModel.HostLine.SAVED_GAMES, saved.hosts.get(0).line);
        assertEquals(2, saved.hosts.get(0).gameCount);
        assertEquals(2, saved.library.size());
        assertFalse(saved.hosts.get(0).online);
    }

    @Test
    public void aHostWithoutALibraryIsOnlineWithoutGamesOrOffline() {
        ComputerDetails fresh = nativeHost("new", "New PC", true);
        HomeModel online = HomeModel.of(rig.repository.cached(Collections.singletonList(fresh)), online("new"));
        assertEquals(HomeModel.HostLine.NO_LIBRARY, online.hosts.get(0).line);
        ComputerDetails off = nativeHost("new", "New PC", false);
        HomeModel offline = HomeModel.of(rig.repository.cached(Collections.singletonList(off)), online());
        assertEquals(HomeModel.HostLine.OFFLINE, offline.hosts.get(0).line);
    }

    @Test
    public void aClassicHostShowsItsAppListWhenOnlineAndIsOfflineOtherwise() {
        rig.classicApps.put("old", Arrays.asList(new NvApp("Desktop", 1, false), new NvApp("Game", 2, false)));
        ComputerDetails old = classicHost("old", "Old PC");
        HomeModel model = HomeModel.of(rig.repository.cached(Collections.singletonList(old)), online("old"));
        assertEquals(HomeModel.HostLine.GAMES, model.hosts.get(0).line);
        assertEquals(2, model.hosts.get(0).gameCount);
        HomeModel away = HomeModel.of(rig.repository.cached(Collections.singletonList(old)), online());
        assertEquals(HomeModel.HostLine.OFFLINE, away.hosts.get(0).line);
    }

    @Test
    public void noHostsIsAnEmptyModel() {
        assertTrue(HomeModel.of(ConsoleSnapshot.empty(), online()).isEmpty());
        assertTrue(HomeModel.empty().isEmpty());
    }
}

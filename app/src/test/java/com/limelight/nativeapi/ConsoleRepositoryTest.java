package com.limelight.nativeapi;

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
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertTrue;

import android.app.Application;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.PairingManager;

import org.junit.After;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class ConsoleRepositoryTest {
    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    private final NativeApiFixtures.FakeTransport transport = new NativeApiFixtures.FakeTransport();
    private final Map<String, List<NvApp>> classicApps = new HashMap<>();
    private final List<RecentsStore.Recent> recents = new ArrayList<>();
    private ExecutorService executor;
    private long now = 5000;
    private ConsoleRepository repository;
    private LibraryCache cache;

    /** In memory, newest first, like the preferences one. */
    private final RecentsStore recentsStore = new RecentsStore() {
        @Override public List<Recent> load() {
            return new ArrayList<>(recents);
        }

        @Override public void record(Recent recent) {
            for (int i = recents.size() - 1; i >= 0; i--) {
                if (recents.get(i).hostUuid.equals(recent.hostUuid) && recents.get(i).appId == recent.appId) {
                    recents.remove(i);
                }
            }
            recents.add(0, recent);
        }

        @Override public void forgetHost(String hostUuid) {
            recents.removeIf(recent -> recent.hostUuid.equals(hostUuid));
        }
    };

    @Before
    public void setUp() throws Exception {
        executor = Executors.newFixedThreadPool(4);
        cache = new FileLibraryCache(temporary.newFolder());
        repository = new ConsoleRepository(transport,
                computer -> HostEndpoint.of(computer, "unique", NativeApiFixtures.crypto()), cache, recentsStore,
                new PreferencesFavouritesStore(RuntimeEnvironment.getApplication()),
                computer -> classicApps.get(computer.uuid), executor, () -> now);
    }

    @After
    public void tearDown() {
        executor.shutdownNow();
    }

    private static NvApp app(int id, String name) {
        NvApp app = new NvApp(name, id, false);
        return app;
    }

    private void tower() {
        transport.reply("tower", "library", ok(library("t1",
                game("t-portal", 11, "Portal", "steam:400", false), game("t-zelda", 12, "Zelda", null, false))));
        transport.reply("tower", "session", ok(NativeApiFixtures.IDLE));
    }

    private void laptop() {
        transport.reply("laptop", "library", ok(library("l1",
                game("l-portal", 5, "portal", "steam:400", true), game("l-alan", 6, "Alan Wake", null, false))));
        transport.reply("laptop", "session", ok(NativeApiFixtures.IDLE));
    }

    @Test
    public void libraryGamesWithOneGameIdFromTwoHostsAreOneTitleAndOthersStayApart() throws Exception {
        tower();
        laptop();
        ConsoleSnapshot snapshot = repository.refresh(Arrays.asList(
                nativeHost("tower", "Tower", true), nativeHost("laptop", "Laptop", true)));
        List<String> titles = new ArrayList<>();
        for (ConsoleSnapshot.Title title : snapshot.titles) {
            titles.add(title.title);
        }
        // Sorted by name; the merged title is called by the copy that is running.
        assertEquals(Arrays.asList("Alan Wake", "portal", "Zelda"), titles);
        ConsoleSnapshot.Title portal = snapshot.titles.get(1);
        assertEquals(2, portal.sources.size());
        assertEquals("laptop", portal.sources.get(0).hostUuid);
        assertTrue(portal.sources.get(0).running);
        assertEquals("tower", portal.sources.get(1).hostUuid);
        assertEquals(ConsoleSnapshot.Status.LIVE, snapshot.host("tower").status);
        assertEquals(5000, snapshot.host("tower").fetchedAtMs);
        assertEquals("steam", portal.sources.get(1).installId == null ? null : "steam");
    }

    @Test
    public void aRenamedHostShowsItsAliasOnHomeLiveCachedAndClassic() throws Exception {
        tower();
        ComputerDetails tower = nativeHost("tower", "Tower", true);
        tower.alias = "Living room";
        ConsoleSnapshot live = repository.refresh(Collections.singletonList(tower));
        assertEquals("Living room", live.host("tower").name);
        assertEquals("Living room", live.titles.get(0).sources.get(0).hostName);

        // Offline: the saved library is read, and the name still is the alias.
        ComputerDetails offline = nativeHost("tower", "Tower", false);
        offline.alias = "Living room";
        ConsoleSnapshot cached = repository.refresh(Collections.singletonList(offline));
        assertEquals(ConsoleSnapshot.Status.CACHED, cached.host("tower").status);
        assertEquals("Living room", cached.host("tower").name);

        ComputerDetails classic = classicHost("classic", "Sunshine");
        classic.alias = "Den";
        classicApps.put("classic", Collections.singletonList(app(1, "Desktop")));
        ConsoleSnapshot snapshot = repository.refresh(Collections.singletonList(classic));
        assertEquals("Den", snapshot.host("classic").name);
        assertEquals("Den", snapshot.titles.get(0).sources.get(0).hostName);
    }

    @Test
    public void aTitleWithoutAGameIdIsNeverMergedAcrossHostsEvenWithTheSameName() throws Exception {
        transport.reply("tower", "library", ok(library("t", game("t-1", 1, "Notepad", null, false))));
        transport.reply("laptop", "library", ok(library("l", game("l-1", 1, "Notepad", null, false))));
        transport.reply("tower", "session", ok(NativeApiFixtures.IDLE));
        transport.reply("laptop", "session", ok(NativeApiFixtures.IDLE));
        ConsoleSnapshot snapshot = repository.refresh(Arrays.asList(
                nativeHost("tower", "Tower", true), nativeHost("laptop", "Laptop", true)));
        assertEquals(2, snapshot.titles.size());
    }

    @Test
    public void aHostThatCannotBeReachedKeepsShowingItsLastLibrary() throws Exception {
        tower();
        ComputerDetails tower = nativeHost("tower", "Tower", true);
        repository.refresh(Collections.singletonList(tower));

        now = 9000;
        transport.replies.clear();
        transport.unreachable("tower");
        ConsoleSnapshot snapshot = repository.refresh(Collections.singletonList(tower));
        ConsoleSnapshot.Host host = snapshot.host("tower");
        assertEquals(ConsoleSnapshot.Status.CACHED, host.status);
        assertEquals(5000, host.fetchedAtMs);
        assertEquals(NativeApiException.Kind.NETWORK, host.error.getKind());
        assertEquals(2, snapshot.titles.size());
        assertEquals("t-zelda", snapshot.titles.get(1).sources.get(0).installId);
        assertEquals("/api/native/v1/cover/t-zelda", snapshot.titles.get(1).sources.get(0).coverPath);
    }

    @Test
    public void anOfflineHostIsReadFromTheCacheWithoutTouchingTheNetwork() throws Exception {
        tower();
        repository.refresh(Collections.singletonList(nativeHost("tower", "Tower", true)));
        transport.calls.clear();

        ConsoleSnapshot snapshot = repository.refresh(Collections.singletonList(nativeHost("tower", "Tower", false)));
        assertTrue(transport.calls.isEmpty());
        assertEquals(ConsoleSnapshot.Status.CACHED, snapshot.host("tower").status);
        assertNull(snapshot.host("tower").error);
        assertEquals(2, snapshot.titles.size());
    }

    @Test
    public void cachedPaintsFromStorageOnlyAndAHostNeverReadIsUnavailable() throws Exception {
        tower();
        repository.refresh(Collections.singletonList(nativeHost("tower", "Tower", true)));
        transport.calls.clear();

        ConsoleSnapshot snapshot = repository.cached(Arrays.asList(
                nativeHost("tower", "Tower", true), nativeHost("fresh", "Fresh", true)));
        assertTrue(transport.calls.isEmpty());
        assertEquals(ConsoleSnapshot.Status.CACHED, snapshot.host("tower").status);
        assertEquals(ConsoleSnapshot.Status.UNAVAILABLE, snapshot.host("fresh").status);
        assertTrue(snapshot.host("fresh").sources.isEmpty());
    }

    @Test
    public void aFailedFirstReadIsUnavailableWithTheReason() throws Exception {
        transport.unreachable("tower");
        ConsoleSnapshot.Host host = repository.refresh(
                Collections.singletonList(nativeHost("tower", "Tower", true))).host("tower");
        assertEquals(ConsoleSnapshot.Status.UNAVAILABLE, host.status);
        assertEquals(NativeApiException.Kind.NETWORK, host.error.getKind());
    }

    @Test
    public void aHostWithoutTheNativeApiListsItsClassicApps() throws Exception {
        classicApps.put("sun", Arrays.asList(app(1, "Desktop"), app(2, "Steam Big Picture")));
        ComputerDetails sunshine = classicHost("sun", "Sunshine PC");
        ConsoleSnapshot snapshot = repository.refresh(Collections.singletonList(sunshine));
        assertTrue(transport.calls.isEmpty());
        ConsoleSnapshot.Host host = snapshot.host("sun");
        assertEquals(ConsoleSnapshot.Status.CLASSIC, host.status);
        assertEquals(2, snapshot.titles.size());
        ConsoleSnapshot.Source desktop = snapshot.titles.get(0).sources.get(0);
        assertEquals("Desktop", desktop.title);
        assertEquals(1, desktop.appId);
        assertNull(desktop.installId);
        assertNull(desktop.coverPath);
        assertEquals("host:sun:1", snapshot.titles.get(0).key);
    }

    @Test
    public void aClassicHostWithoutAKnownAppListIsUnavailableAndAnUnpairedOneIsLeftOut() throws Exception {
        ComputerDetails sunshine = classicHost("sun", "Sunshine PC");
        ComputerDetails stranger = classicHost("stranger", "Stranger");
        stranger.pairState = PairingManager.PairState.NOT_PAIRED;
        classicApps.put("stranger", Collections.singletonList(app(1, "Desktop")));
        ConsoleSnapshot snapshot = repository.refresh(Arrays.asList(sunshine, stranger));
        assertEquals(ConsoleSnapshot.Status.UNAVAILABLE, snapshot.host("sun").status);
        assertNull(snapshot.host("stranger"));
        assertTrue(snapshot.titles.isEmpty());
    }

    @Test
    public void aRunningOrSuspendedSessionOnAnyHostIsContinuePlayingRunningFirst() throws Exception {
        transport.reply("tower", "library", ok(library("t", game("t-portal", 11, "Portal", "steam:400", false))));
        transport.reply("tower", "session", ok(session("suspended", "you",
                "{\"install_id\":\"t-portal\",\"app_id\":11,\"title\":\"Portal\"}", "[{\"id\":\"resume\",\"allowed\":true}]")));
        transport.reply("laptop", "library", ok(library("l", game("l-alan", 6, "Alan Wake", null, true))));
        transport.reply("laptop", "session", ok(session("streaming", "other_device",
                "{\"install_id\":\"l-alan\",\"app_id\":6,\"title\":\"Alan Wake\"}", "[]")));
        ConsoleSnapshot snapshot = repository.refresh(Arrays.asList(
                nativeHost("tower", "Tower", true), nativeHost("laptop", "Laptop", true)));
        assertEquals(2, snapshot.continuePlaying.size());
        ConsoleSnapshot.Continue first = snapshot.continuePlaying.get(0);
        assertEquals(ConsoleSnapshot.ContinueKind.RUNNING, first.kind);
        assertEquals("Laptop", first.hostName);
        assertEquals(NativeSession.Owner.OTHER_DEVICE, first.session.owner);
        assertEquals("l-alan", first.source.installId);
        ConsoleSnapshot.Continue second = snapshot.continuePlaying.get(1);
        assertEquals(ConsoleSnapshot.ContinueKind.SUSPENDED, second.kind);
        assertTrue(second.session.allows(NativeSession.ActionId.RESUME));
    }

    @Test
    public void aSessionThatFailsToLoadDoesNotCostTheLibrary() throws Exception {
        transport.reply("tower", "library", ok(library("t", game("t-portal", 11, "Portal", null, false))));
        transport.reply("tower", "session", "{\"error\":{\"kind\":\"status\",\"message\":\"boom\",\"http_status\":500}}");
        ConsoleSnapshot snapshot = repository.refresh(Collections.singletonList(nativeHost("tower", "Tower", true)));
        assertEquals(ConsoleSnapshot.Status.LIVE, snapshot.host("tower").status);
        assertNull(snapshot.host("tower").session);
        assertEquals(1, snapshot.titles.size());
        assertTrue(snapshot.continuePlaying.isEmpty());
    }

    @Test
    public void aRunningGameOnAClassicHostIsContinuePlayingToo() throws Exception {
        classicApps.put("sun", Arrays.asList(app(1, "Desktop"), app(2, "Steam Big Picture")));
        ComputerDetails sunshine = classicHost("sun", "Sunshine PC");
        sunshine.runningGameId = 2;
        ConsoleSnapshot snapshot = repository.refresh(Collections.singletonList(sunshine));
        assertEquals(1, snapshot.continuePlaying.size());
        assertEquals("Steam Big Picture", snapshot.continuePlaying.get(0).title);
        assertEquals(ConsoleSnapshot.ContinueKind.RUNNING, snapshot.continuePlaying.get(0).kind);
        assertNull(snapshot.continuePlaying.get(0).session);
    }

    @Test
    public void recentsFollowTheSessionsNewestFirstWithoutRepeatingARunningGame() throws Exception {
        transport.reply("tower", "library", ok(library("t",
                game("t-portal", 11, "Portal", null, false), game("t-zelda", 12, "Zelda", null, false),
                game("t-doom", 13, "Doom", null, false))));
        transport.reply("tower", "session", ok(session("streaming", "you",
                "{\"install_id\":\"t-doom\",\"app_id\":13,\"title\":\"Doom\"}", "[]")));
        ComputerDetails tower = nativeHost("tower", "Tower", true);
        now = 100;
        repository.recordLaunch(tower, app(11, "Portal"));
        now = 200;
        repository.recordLaunch(tower, app(13, "Doom"));
        now = 300;
        repository.recordLaunch(tower, app(12, "Zelda"));
        now = 400;
        repository.recordLaunch(tower, app(11, "Portal"));

        ConsoleSnapshot snapshot = repository.refresh(Collections.singletonList(tower));
        List<String> order = new ArrayList<>();
        for (ConsoleSnapshot.Continue entry : snapshot.continuePlaying) {
            order.add(entry.kind + ":" + entry.title);
        }
        assertEquals(Arrays.asList("RUNNING:Doom", "RECENT:Portal", "RECENT:Zelda"), order);
        assertEquals(400, snapshot.continuePlaying.get(1).playedAtMs);
        assertEquals("t-portal", snapshot.continuePlaying.get(1).source.installId);
    }

    @Test
    public void onlyStreamLaunchesAreRecentAndAGameTheHostNoLongerHasIsDropped() throws Exception {
        transport.reply("tower", "library", ok(library("t", game("t-portal", 11, "Portal", null, false))));
        transport.reply("tower", "session", ok(NativeApiFixtures.IDLE));
        ComputerDetails tower = nativeHost("tower", "Tower", true);
        NvApp inputOnly = app(11, "Portal");
        inputOnly.setSmallStreamInputOnly(true);
        repository.recordLaunch(tower, inputOnly);
        assertTrue(recents.isEmpty());

        repository.recordLaunch(tower, app(99, "Uninstalled"));
        repository.recordLaunch(tower, app(11, "Portal"));
        ConsoleSnapshot snapshot = repository.refresh(Collections.singletonList(tower));
        assertEquals(1, snapshot.continuePlaying.size());
        assertEquals("Portal", snapshot.continuePlaying.get(0).title);
    }

    @Test
    public void recentsOfAHostWithNoLibraryYetAreKeptAndOfAnUnknownHostLeftOut() throws Exception {
        transport.unreachable("tower");
        ComputerDetails tower = nativeHost("tower", "Tower", true);
        repository.recordLaunch(tower, app(11, "Portal"));
        repository.recordLaunch(nativeHost("gone", "Gone", true), app(1, "Elsewhere"));
        ConsoleSnapshot snapshot = repository.refresh(Collections.singletonList(tower));
        assertEquals(1, snapshot.continuePlaying.size());
        assertEquals("Portal", snapshot.continuePlaying.get(0).title);
        assertNull(snapshot.continuePlaying.get(0).source);
    }

    @Test
    public void forgettingAHostDropsItsCacheAndRecents() throws Exception {
        tower();
        ComputerDetails tower = nativeHost("tower", "Tower", true);
        repository.refresh(Collections.singletonList(tower));
        repository.recordLaunch(tower, app(11, "Portal"));
        assertNotNull(cache.load("tower"));

        repository.forgetHost("tower");
        assertNull(cache.load("tower"));
        assertTrue(recents.isEmpty());
    }

    @Test
    public void aHostWhoseEndpointIsMissingFallsBackToTheCache() throws Exception {
        tower();
        ComputerDetails tower = nativeHost("tower", "Tower", true);
        repository.refresh(Collections.singletonList(tower));
        tower.serverCert = null;
        transport.calls.clear();
        ConsoleSnapshot snapshot = repository.refresh(Collections.singletonList(tower));
        assertTrue(transport.calls.isEmpty());
        assertEquals(ConsoleSnapshot.Status.CACHED, snapshot.host("tower").status);
        assertFalse(snapshot.titles.isEmpty());
        assertSame(snapshot.hosts.get(0), snapshot.host("tower"));
    }

    private static String capabilitiesReply(boolean hostControl) {
        return ok("{\"api_version\":1,\"host\":{\"name\":\"Tower\",\"version\":\"3.0.0\",\"unique_id\":\"U\"},"
                + "\"features\":[\"library\"],\"permissions\":{\"list\":true,\"view\":true,"
                + "\"launch\":true,\"host_control\":" + hostControl + "}}");
    }

    @Test
    public void capabilitiesCarryHostControlOfThisDeviceAndAreNullWhereTheyCannotBeRead() {
        transport.reply("tower", "capabilities", capabilitiesReply(true));
        transport.reply("laptop", "capabilities", capabilitiesReply(false));
        assertTrue(repository.capabilities(nativeHost("tower", "Tower", true)).hostControl);
        assertFalse(repository.capabilities(nativeHost("laptop", "Laptop", true)).hostControl);
        // A classic host has no native API to ask, and one that does not answer has no permissions to show.
        assertNull(repository.capabilities(classicHost("old", "Old PC")));
        assertNull(repository.capabilities(nativeHost("gone", "Gone", true)));
        assertFalse(transport.calls.contains("old|capabilities"));
    }

    private static final String FOUND = "{\"install_id\":null,\"title\":\"Celeste\",\"game_id\":null,"
            + "\"store\":{\"source\":\"gog\",\"name\":\"GOG\"},\"installed\":true,\"added\":false}";
    private static final String FOUND_TOO = "{\"title\":\"Hades\",\"store\":{\"source\":\"epic\"},\"added\":false}";
    private static final String PLAYED = "{\"install_id\":\"t-hades\",\"app_id\":13,\"title\":\"Hades II\","
            + "\"store\":{\"source\":\"epic\",\"id\":\"h2\",\"name\":\"Epic Games\"},\"installed\":false,"
            + "\"last_played\":1761300000}";

    private void unaddedTower(boolean advertised) {
        transport.reply("tower", "capabilities", ok("{\"api_version\":1,\"features\":[\"library\""
                + (advertised ? ",\"library.unadded\"" : "") + "],\"permissions\":{\"list\":true}}"));
        transport.reply("tower", "library", ok(library("t1", game("t-portal", 11, "Portal", "steam:400", false),
                PLAYED, FOUND, FOUND_TOO)));
        transport.reply("tower", "session", ok(NativeApiFixtures.IDLE));
    }

    @Test
    public void aHostWithTheUnaddedFeatureIsAskedForThemAndKeepsTheFeatureOffline() throws Exception {
        unaddedTower(true);
        ConsoleSnapshot snapshot = repository.refresh(Collections.singletonList(nativeHost("tower", "Tower", true)));
        assertEquals("include=unadded", transport.arguments.get(transport.calls.indexOf("tower|library")));
        assertTrue(snapshot.host("tower").has(NativeCapabilities.LIBRARY_UNADDED));

        // Read once while the repository lives.
        repository.refresh(Collections.singletonList(nativeHost("tower", "Tower", true)));
        assertEquals(1, Collections.frequency(transport.calls, "tower|capabilities"));

        transport.calls.clear();
        ConsoleSnapshot offline = repository.refresh(Collections.singletonList(nativeHost("tower", "Tower", false)));
        assertTrue(transport.calls.isEmpty());
        assertEquals(ConsoleSnapshot.Status.CACHED, offline.host("tower").status);
        assertTrue(offline.host("tower").has(NativeCapabilities.LIBRARY_UNADDED));
    }

    @Test
    public void aHostWithoutTheFeatureOrWithoutCapabilitiesIsAskedForItsLibraryOnly() throws Exception {
        unaddedTower(false);
        ConsoleSnapshot snapshot = repository.refresh(Collections.singletonList(nativeHost("tower", "Tower", true)));
        assertEquals("", transport.arguments.get(transport.calls.indexOf("tower|library")));
        assertFalse(snapshot.host("tower").has(NativeCapabilities.LIBRARY_UNADDED));

        transport.replies.remove("tower|capabilities");
        ConsoleRepository fresh = new ConsoleRepository(transport,
                computer -> HostEndpoint.of(computer, "unique", NativeApiFixtures.crypto()), cache, recentsStore,
                new PreferencesFavouritesStore(RuntimeEnvironment.getApplication()),
                computer -> classicApps.get(computer.uuid), executor, () -> now);
        transport.calls.clear();
        transport.arguments.clear();
        ConsoleSnapshot unanswered = fresh.refresh(Collections.singletonList(nativeHost("tower", "Tower", true)));
        assertEquals(ConsoleSnapshot.Status.LIVE, unanswered.host("tower").status);
        assertEquals("", transport.arguments.get(transport.calls.indexOf("tower|library")));
        assertTrue(unanswered.host("tower").features.isEmpty());
    }

    @Test
    public void theStoreAndThePlayedAndAddedStateReachTheTitles() throws Exception {
        unaddedTower(true);
        ConsoleSnapshot snapshot = repository.refresh(Collections.singletonList(nativeHost("tower", "Tower", true)));
        Map<String, ConsoleSnapshot.Source> byTitle = new HashMap<>();
        for (ConsoleSnapshot.Title title : snapshot.titles) {
            byTitle.put(title.title, title.sources.get(0));
        }
        assertEquals(4, snapshot.titles.size());
        ConsoleSnapshot.Source hades = byTitle.get("Hades II");
        assertEquals("epic", hades.storeSource);
        assertEquals("Epic Games", hades.storeName);
        assertEquals(Long.valueOf(1761300000L), hades.lastPlayed);
        assertEquals(Boolean.FALSE, hades.installed);
        assertTrue(hades.added);
        assertEquals("steam", byTitle.get("Portal").storeSource);
        assertNull(byTitle.get("Portal").lastPlayed);
        // Two found games with no ids at all are two titles, not one under app id 0.
        assertFalse(byTitle.get("Celeste").added);
        assertFalse(byTitle.get("Hades").added);
        assertEquals("", byTitle.get("Celeste").installId);
    }
}

package com.limelight.nativeapi;

import android.content.Context;

import com.limelight.LimeLog;
import com.limelight.binding.PlatformBinding;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.PairingManager;

import org.json.JSONException;
import org.json.JSONObject;

import java.io.File;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;

/**
 * The data behind the console's Home: the libraries of all paired hosts in one list, and the
 * games to continue. A host with the native API contributes its library (cached for when it is
 * offline); any other host contributes its classic Moonlight app list, as before.
 *
 * {@link #refresh} blocks on the network, so it runs on a worker thread; {@link #cached} does
 * not touch the network and is for the first paint.
 */
public final class ConsoleRepository {
    public interface EndpointFactory {
        /** The endpoint of the host, or null when it cannot be reached with what is known. */
        HostEndpoint endpoint(ComputerDetails computer);
    }

    public interface Clock {
        long nowMs();
    }

    private static final int MAX_RECENTS_SHOWN = 10;

    private final NativeApiTransport transport;
    private final EndpointFactory endpoints;
    private final LibraryCache cache;
    private final RecentsStore recents;
    private final FavouritesStore favourites;
    private final ClassicAppSource classic;
    private final ExecutorService executor;
    private final Clock clock;
    private final Map<String, List<String>> features = new java.util.concurrent.ConcurrentHashMap<>();

    public ConsoleRepository(NativeApiTransport transport, EndpointFactory endpoints, LibraryCache cache,
                             RecentsStore recents, FavouritesStore favourites, ClassicAppSource classic, ExecutorService executor,
                             Clock clock) {
        this.transport = transport;
        this.endpoints = endpoints;
        this.cache = cache;
        this.recents = recents;
        this.favourites = favourites;
        this.classic = classic;
        this.executor = executor;
        this.clock = clock;
    }

    /** The repository of the app: the Rust client, the app's own cache directories and clock. */
    public static ConsoleRepository create(Context context, String uniqueId) {
        Context app = context.getApplicationContext();
        return new ConsoleRepository(new JniNativeApiTransport(),
                computer -> HostEndpoint.of(computer, uniqueId, PlatformBinding.getCryptoProvider(app)),
                new FileLibraryCache(libraryDirectory(app)),
                new PreferencesRecentsStore(app), new PreferencesFavouritesStore(app), new CachedAppListSource(app.getCacheDir()),
                Executors.newCachedThreadPool(), System::currentTimeMillis);
    }

    private static File libraryDirectory(Context context) {
        return new File(context.getFilesDir(), "console-library");
    }

    /** True when the app holds the library of some host, which is when Home has something to show. */
    public static boolean hasAnyLibrary(Context context) {
        String[] files = libraryDirectory(context.getApplicationContext()).list((dir, name) -> name.endsWith(".json"));
        return files != null && files.length > 0;
    }

    /** True when the app holds the last library of this host. */
    public static boolean hasLibrary(Context context, String hostUuid) {
        return new FileLibraryCache(libraryDirectory(context.getApplicationContext())).load(hostUuid) != null;
    }

    /** Drops what the app stores for a removed host without needing a repository. */
    public static void forgetHost(Context context, String hostUuid) {
        Context app = context.getApplicationContext();
        new FileLibraryCache(libraryDirectory(app)).forget(hostUuid);
        new PreferencesRecentsStore(app).forgetHost(hostUuid);
        new PreferencesFavouritesStore(app).forgetHost(hostUuid);
    }

    /** The snapshot from what is stored: no network. Hosts with a native library show CACHED. */
    public ConsoleSnapshot cached(List<ComputerDetails> computers) {
        List<ConsoleSnapshot.Host> hosts = new ArrayList<>();
        for (ComputerDetails computer : computers) {
            ConsoleSnapshot.Host host = hostOf(computer, false);
            if (host != null) {
                hosts.add(host);
            }
        }
        return assemble(hosts);
    }

    /** Reads every reachable host (in parallel) and returns the snapshot; stores the libraries it read. */
    public ConsoleSnapshot refresh(List<ComputerDetails> computers) throws InterruptedException {
        List<Future<ConsoleSnapshot.Host>> pending = new ArrayList<>();
        for (ComputerDetails computer : computers) {
            pending.add(executor.submit(() -> hostOf(computer, true)));
        }
        List<ConsoleSnapshot.Host> hosts = new ArrayList<>();
        for (Future<ConsoleSnapshot.Host> future : pending) {
            try {
                ConsoleSnapshot.Host host = future.get();
                if (host != null) {
                    hosts.add(host);
                }
            } catch (ExecutionException e) {
                LimeLog.warning("Console library failed: " + e.getCause());
            }
        }
        return assemble(hosts);
    }

    /**
     * The permissions of this device on the host, read now (blocks); null for a host without the native API
     * and when the host does not answer.
     */
    public NativeCapabilities capabilities(ComputerDetails computer) {
        if (!computer.caps.has(HostCapabilities.Feature.NATIVE_API)) {
            return null;
        }
        HostEndpoint endpoint = endpoints.endpoint(computer);
        if (endpoint == null) {
            return null;
        }
        try {
            return new NativeApiClient(transport, endpoint).capabilities();
        } catch (NativeApiException e) {
            LimeLog.info("No capabilities from " + computer.name + ": " + e.getMessage());
            return null;
        }
    }

    /**
     * Asks the host to add a game it found but has not added (blocks; only for a host with
     * {@link NativeCapabilities#LIBRARY_ADD}) and keeps the library it answers with. Returns the game as the host now
     * lists it, or null when the answer does not list it as added.
     */
    public NativeLibrary.Game addToLibrary(ComputerDetails computer, String gameId) throws NativeApiException {
        HostEndpoint endpoint = endpoints.endpoint(computer);
        if (endpoint == null) {
            throw new NativeApiException(NativeApiException.Kind.UNAVAILABLE, computer.name + " has no address");
        }
        NativeLibrary library = new NativeApiClient(transport, endpoint).addToLibrary(gameId);
        List<String> known = features.get(computer.uuid);
        try {
            cache.store(computer.uuid, new LibraryCache.Entry(library.toJson().toString(), clock.nowMs(),
                    known == null ? Collections.emptyList() : known));
        } catch (JSONException e) {
            throw new NativeApiException(NativeApiException.Kind.MALFORMED, e.getMessage());
        }
        for (NativeLibrary.Game game : library.games) {
            if (gameId.equals(game.gameId) && game.added) return game;
        }
        return null;
    }

    /** Remembers that this device streamed the app; call when a stream starts. */
    public void recordLaunch(ComputerDetails computer, NvApp app) {
        if (app.getRole() == NvApp.Role.STREAM) {
            recents.record(RecentsStore.Recent.of(computer, app, clock.nowMs()));
        }
    }

    /** Drops what is stored for a host that was removed. */
    public void forgetHost(String hostUuid) {
        cache.forget(hostUuid);
        recents.forgetHost(hostUuid);
        favourites.forgetHost(hostUuid);
    }

    /**
     * True when the game is starred on that host. {@code key} is {@link ConsoleSnapshot.Source#stableId()} when
     * the library lists the game, else empty.
     */
    public boolean isFavourite(String hostUuid, String key, int appId) {
        return favourites.isFavourite(hostUuid, key, appId);
    }

    /** {@link #isFavourite(String, String, int)} without a stable id: by app id. */
    public boolean isFavourite(String hostUuid, int appId) {
        return favourites.isFavourite(hostUuid, "", appId);
    }

    /** Stars or unstars a game; the names are kept for when the host's library is not at hand. */
    public void setFavourite(String hostUuid, String hostName, String key, int appId, String title, boolean starred) {
        favourites.set(new FavouritesStore.Favourite(hostUuid, hostName, key, appId, title), starred);
    }

    /** {@link #setFavourite(String, String, String, int, String, boolean)} without a stable id: by app id. */
    public void setFavourite(String hostUuid, String hostName, int appId, String title, boolean starred) {
        setFavourite(hostUuid, hostName, "", appId, title, starred);
    }

    private ConsoleSnapshot.Host hostOf(ComputerDetails computer, boolean network) {
        if (computer.caps.has(HostCapabilities.Feature.NATIVE_API)) {
            return nativeHost(computer, network);
        }
        if (computer.pairState != PairingManager.PairState.PAIRED) {
            return null;
        }
        List<NvApp> apps = classic.apps(computer);
        if (apps == null) {
            return new ConsoleSnapshot.Host(computer.uuid, computer.displayName(), ConsoleSnapshot.Status.UNAVAILABLE, 0,
                    null, null, Collections.emptyList());
        }
        List<ConsoleSnapshot.Source> sources = new ArrayList<>();
        for (NvApp app : apps) {
            sources.add(new ConsoleSnapshot.Source(computer.uuid, computer.displayName(), app.getAppId(), null,
                    app.getAppName(), null, computer.hasRunningGame() && app.matchesRunningApp(computer.runningGameId,
                    computer.runningGameUuid), null, app.getArtVersion(), null, app.getAppUuid(),
                    app.isHdrSupported()));
        }
        return new ConsoleSnapshot.Host(computer.uuid, computer.displayName(), ConsoleSnapshot.Status.CLASSIC, 0, null, null,
                sources);
    }

    private ConsoleSnapshot.Host nativeHost(ComputerDetails computer, boolean network) {
        NativeApiException failure = null;
        if (network && computer.state == ComputerDetails.State.ONLINE) {
            HostEndpoint endpoint = endpoints.endpoint(computer);
            if (endpoint != null) {
                NativeApiClient client = new NativeApiClient(transport, endpoint);
                try {
                    List<String> features = features(computer, client);
                    NativeLibrary library = client.library(features.contains(NativeCapabilities.LIBRARY_UNADDED));
                    long now = clock.nowMs();
                    cache.store(computer.uuid, new LibraryCache.Entry(library.toJson().toString(), now, features));
                    NativeSession session = null;
                    try {
                        session = client.session();
                    } catch (NativeApiException e) {
                        // Home still has the library; Continue playing just lacks this host's session.
                        LimeLog.info("No session from " + computer.name + ": " + e.getMessage());
                    }
                    return new ConsoleSnapshot.Host(computer.uuid, computer.displayName(), ConsoleSnapshot.Status.LIVE, now,
                            null, session, sourcesOf(computer, library), new HashSet<>(features));
                } catch (NativeApiException e) {
                    failure = e;
                } catch (JSONException e) {
                    failure = new NativeApiException(NativeApiException.Kind.MALFORMED, e.getMessage());
                }
            }
        }
        LibraryCache.Entry stored = cache.load(computer.uuid);
        if (stored != null) {
            try {
                NativeLibrary library = NativeLibrary.parse(new JSONObject(stored.libraryJson));
                return new ConsoleSnapshot.Host(computer.uuid, computer.displayName(), ConsoleSnapshot.Status.CACHED,
                        stored.fetchedAtMs, failure, null, sourcesOf(computer, library), new HashSet<>(stored.features));
            } catch (JSONException e) {
                LimeLog.warning("Unreadable cached library of " + computer.name + ": " + e.getMessage());
            }
        }
        return new ConsoleSnapshot.Host(computer.uuid, computer.displayName(), ConsoleSnapshot.Status.UNAVAILABLE, 0, failure,
                null, Collections.emptyList());
    }

    /**
     * The features the host advertises, read once per host while this repository lives (a host changes them only with
     * an update). A host that does not answer has none this time; its library still loads.
     */
    private List<String> features(ComputerDetails computer, NativeApiClient client) {
        List<String> known = features.get(computer.uuid);
        if (known != null) return known;
        try {
            known = client.capabilities().features;
            features.put(computer.uuid, known);
            return known;
        } catch (NativeApiException e) {
            LimeLog.info("No capabilities from " + computer.name + ": " + e.getMessage());
            return Collections.emptyList();
        }
    }

    private static List<ConsoleSnapshot.Source> sourcesOf(ComputerDetails computer, NativeLibrary library) {
        List<ConsoleSnapshot.Source> sources = new ArrayList<>();
        for (NativeLibrary.Game game : library.games) {
            sources.add(new ConsoleSnapshot.Source(computer.uuid, computer.displayName(), game.appId,
                    game.installId == null ? "" : game.installId, game.title, game.gameId, game.running,
                    game.coverPath, game.coverVersion, game.installed, "", false, game.storeSource, game.storeName,
                    game.added, game.lastPlayed, game.launchable));
        }
        return sources;
    }

    private ConsoleSnapshot assemble(List<ConsoleSnapshot.Host> hosts) {
        return new ConsoleSnapshot(titles(hosts), hosts, continuePlaying(hosts), starred(hosts));
    }

    private List<ConsoleSnapshot.Starred> starred(List<ConsoleSnapshot.Host> hosts) {
        List<ConsoleSnapshot.Starred> starred = new ArrayList<>();
        for (FavouritesStore.Favourite favourite : favourites.load()) {
            ConsoleSnapshot.Host host = hostOrNull(hosts, favourite.hostUuid);
            if (host == null) {
                continue;
            }
            ConsoleSnapshot.Source source = findStarred(host, favourite);
            // Same rule as recents: a known library without the game means it is gone from the host. The star
            // stays stored, so the game comes back with a reinstall.
            if (source == null && host.status != ConsoleSnapshot.Status.UNAVAILABLE) {
                continue;
            }
            int appId = source != null ? source.appId : favourite.appId;
            if (source != null && (source.appId != favourite.appId || !source.stableId().equals(favourite.key))) {
                // The host renumbered its apps or only now gave an id: follow the game and keep the star on it.
                favourites.set(new FavouritesStore.Favourite(host.uuid, host.name, source.stableId(), source.appId,
                        source.title), true);
            }
            starred.add(new ConsoleSnapshot.Starred(host.uuid, host.name, appId,
                    source != null ? source.title : favourite.title, source));
        }
        return starred;
    }

    /** Merges the titles of all hosts: a native game id joins copies on different hosts, nothing else does. */
    private List<ConsoleSnapshot.Title> titles(List<ConsoleSnapshot.Host> hosts) {
        Map<String, List<ConsoleSnapshot.Source>> byKey = new LinkedHashMap<>();
        for (ConsoleSnapshot.Host host : hosts) {
            for (ConsoleSnapshot.Source source : host.sources) {
                // A game the host has not added has no app id yet: its name tells it apart.
                String key = source.gameId != null ? "game:" + source.gameId
                        : !source.added ? "found:" + source.hostUuid + ":" + source.title
                        : "host:" + source.hostUuid + ":" + source.appId;
                List<ConsoleSnapshot.Source> sources = byKey.get(key);
                if (sources == null) {
                    sources = new ArrayList<>();
                    byKey.put(key, sources);
                }
                sources.add(source);
            }
        }
        List<ConsoleSnapshot.Title> titles = new ArrayList<>();
        for (Map.Entry<String, List<ConsoleSnapshot.Source>> entry : byKey.entrySet()) {
            List<ConsoleSnapshot.Source> sources = entry.getValue();
            // Stable: running copies first, otherwise the order of the hosts.
            Collections.sort(sources, (a, b) -> Boolean.compare(b.running, a.running));
            titles.add(new ConsoleSnapshot.Title(entry.getKey(), sources.get(0).title, sources));
        }
        Collections.sort(titles, TITLE_ORDER);
        return titles;
    }

    private static final Comparator<ConsoleSnapshot.Title> TITLE_ORDER = (a, b) -> {
        int byName = a.title.toLowerCase(Locale.ROOT).compareTo(b.title.toLowerCase(Locale.ROOT));
        return byName != 0 ? byName : a.key.compareTo(b.key);
    };

    private List<ConsoleSnapshot.Continue> continuePlaying(List<ConsoleSnapshot.Host> hosts) {
        List<ConsoleSnapshot.Continue> running = new ArrayList<>();
        List<ConsoleSnapshot.Continue> suspended = new ArrayList<>();
        List<ConsoleSnapshot.Continue> older = new ArrayList<>();
        for (ConsoleSnapshot.Host host : hosts) {
            NativeSession session = host.session;
            if (session != null && session.isActive()) {
                ConsoleSnapshot.Source source = find(host, session.app.installId, session.app.appId);
                ConsoleSnapshot.Continue entry = new ConsoleSnapshot.Continue(
                        session.state == NativeSession.State.SUSPENDED ? ConsoleSnapshot.ContinueKind.SUSPENDED
                                : ConsoleSnapshot.ContinueKind.RUNNING,
                        host.uuid, host.name, session.app.appId, session.app.title, session, source, 0);
                (session.state == NativeSession.State.SUSPENDED ? suspended : running).add(entry);
            } else if (host.status == ConsoleSnapshot.Status.CLASSIC) {
                for (ConsoleSnapshot.Source source : host.sources) {
                    if (source.running) {
                        running.add(new ConsoleSnapshot.Continue(ConsoleSnapshot.ContinueKind.RUNNING, host.uuid,
                                host.name, source.appId, source.title, null, source, 0));
                    }
                }
            }
        }
        int shown = 0;
        for (RecentsStore.Recent recent : recents.load()) {
            ConsoleSnapshot.Host host = hostOrNull(hosts, recent.hostUuid);
            if (host == null || isActiveEntry(running, recent) || isActiveEntry(suspended, recent)) {
                continue;
            }
            ConsoleSnapshot.Source source = find(host, null, recent.appId);
            // A known library without the game means it was uninstalled or removed from the host.
            if (source == null && host.status != ConsoleSnapshot.Status.UNAVAILABLE) {
                continue;
            }
            older.add(new ConsoleSnapshot.Continue(ConsoleSnapshot.ContinueKind.RECENT, host.uuid, host.name,
                    recent.appId, source != null ? source.title : recent.title, null, source, recent.playedAtMs));
            if (++shown == MAX_RECENTS_SHOWN) {
                break;
            }
        }
        List<ConsoleSnapshot.Continue> all = new ArrayList<>(running);
        all.addAll(suspended);
        all.addAll(older);
        return all;
    }

    private static boolean isActiveEntry(List<ConsoleSnapshot.Continue> entries, RecentsStore.Recent recent) {
        for (ConsoleSnapshot.Continue entry : entries) {
            if (entry.hostUuid.equals(recent.hostUuid) && entry.appId == recent.appId) {
                return true;
            }
        }
        return false;
    }

    private static ConsoleSnapshot.Host hostOrNull(List<ConsoleSnapshot.Host> hosts, String uuid) {
        for (ConsoleSnapshot.Host host : hosts) {
            if (host.uuid.equals(uuid)) {
                return host;
            }
        }
        return null;
    }

    /** By the stored stable id first; the app id only for a star or a game that has no id. */
    private static ConsoleSnapshot.Source findStarred(ConsoleSnapshot.Host host, FavouritesStore.Favourite favourite) {
        for (ConsoleSnapshot.Source source : host.sources) {
            String id = source.stableId();
            if (favourite.key.isEmpty() || id.isEmpty() ? source.appId == favourite.appId : id.equals(favourite.key)) {
                return source;
            }
        }
        return null;
    }

    private static ConsoleSnapshot.Source find(ConsoleSnapshot.Host host, String installId, int appId) {
        for (ConsoleSnapshot.Source source : host.sources) {
            if (installId != null ? installId.equals(source.installId) : source.appId == appId) {
                return source;
            }
        }
        return null;
    }
}

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
    private final ClassicAppSource classic;
    private final ExecutorService executor;
    private final Clock clock;

    public ConsoleRepository(NativeApiTransport transport, EndpointFactory endpoints, LibraryCache cache,
                             RecentsStore recents, ClassicAppSource classic, ExecutorService executor,
                             Clock clock) {
        this.transport = transport;
        this.endpoints = endpoints;
        this.cache = cache;
        this.recents = recents;
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
                new PreferencesRecentsStore(app), new CachedAppListSource(app.getCacheDir()),
                Executors.newCachedThreadPool(), System::currentTimeMillis);
    }

    private static File libraryDirectory(Context context) {
        return new File(context.getFilesDir(), "console-library");
    }

    /** Drops what the app stores for a removed host without needing a repository. */
    public static void forgetHost(Context context, String hostUuid) {
        Context app = context.getApplicationContext();
        new FileLibraryCache(libraryDirectory(app)).forget(hostUuid);
        new PreferencesRecentsStore(app).forgetHost(hostUuid);
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
            return new ConsoleSnapshot.Host(computer.uuid, computer.name, ConsoleSnapshot.Status.UNAVAILABLE, 0,
                    null, null, Collections.emptyList());
        }
        List<ConsoleSnapshot.Source> sources = new ArrayList<>();
        for (NvApp app : apps) {
            sources.add(new ConsoleSnapshot.Source(computer.uuid, computer.name, app.getAppId(), null,
                    app.getAppName(), null, computer.hasRunningGame() && app.matchesRunningApp(computer.runningGameId,
                    computer.runningGameUuid), null, app.getArtVersion(), null, app.getAppUuid(),
                    app.isHdrSupported()));
        }
        return new ConsoleSnapshot.Host(computer.uuid, computer.name, ConsoleSnapshot.Status.CLASSIC, 0, null, null,
                sources);
    }

    private ConsoleSnapshot.Host nativeHost(ComputerDetails computer, boolean network) {
        NativeApiException failure = null;
        if (network && computer.state == ComputerDetails.State.ONLINE) {
            HostEndpoint endpoint = endpoints.endpoint(computer);
            if (endpoint != null) {
                NativeApiClient client = new NativeApiClient(transport, endpoint);
                try {
                    NativeLibrary library = client.library();
                    long now = clock.nowMs();
                    cache.store(computer.uuid, new LibraryCache.Entry(library.toJson().toString(), now));
                    NativeSession session = null;
                    try {
                        session = client.session();
                    } catch (NativeApiException e) {
                        // Home still has the library; Continue playing just lacks this host's session.
                        LimeLog.info("No session from " + computer.name + ": " + e.getMessage());
                    }
                    return new ConsoleSnapshot.Host(computer.uuid, computer.name, ConsoleSnapshot.Status.LIVE, now,
                            null, session, sourcesOf(computer, library));
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
                return new ConsoleSnapshot.Host(computer.uuid, computer.name, ConsoleSnapshot.Status.CACHED,
                        stored.fetchedAtMs, failure, null, sourcesOf(computer, library));
            } catch (JSONException e) {
                LimeLog.warning("Unreadable cached library of " + computer.name + ": " + e.getMessage());
            }
        }
        return new ConsoleSnapshot.Host(computer.uuid, computer.name, ConsoleSnapshot.Status.UNAVAILABLE, 0, failure,
                null, Collections.emptyList());
    }

    private static List<ConsoleSnapshot.Source> sourcesOf(ComputerDetails computer, NativeLibrary library) {
        List<ConsoleSnapshot.Source> sources = new ArrayList<>();
        for (NativeLibrary.Game game : library.games) {
            sources.add(new ConsoleSnapshot.Source(computer.uuid, computer.name, game.appId, game.installId,
                    game.title, game.gameId, game.running, game.coverPath, game.coverVersion, game.installed, "",
                    false));
        }
        return sources;
    }

    private ConsoleSnapshot assemble(List<ConsoleSnapshot.Host> hosts) {
        return new ConsoleSnapshot(titles(hosts), hosts, continuePlaying(hosts));
    }

    /** Merges the titles of all hosts: a native game id joins copies on different hosts, nothing else does. */
    private List<ConsoleSnapshot.Title> titles(List<ConsoleSnapshot.Host> hosts) {
        Map<String, List<ConsoleSnapshot.Source>> byKey = new LinkedHashMap<>();
        for (ConsoleSnapshot.Host host : hosts) {
            for (ConsoleSnapshot.Source source : host.sources) {
                String key = source.gameId != null ? "game:" + source.gameId
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

    private static ConsoleSnapshot.Source find(ConsoleSnapshot.Host host, String installId, int appId) {
        for (ConsoleSnapshot.Source source : host.sources) {
            if (installId != null ? installId.equals(source.installId) : source.appId == appId) {
                return source;
            }
        }
        return null;
    }
}

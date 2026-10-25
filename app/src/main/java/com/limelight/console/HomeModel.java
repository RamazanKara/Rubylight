package com.limelight.console;

import com.limelight.nativeapi.ConsoleSnapshot;
import com.limelight.nativeapi.NativeCapabilities;
import com.limelight.nativeapi.NativeSession;
import com.limelight.nvstream.http.ComputerDetails;

import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * What Home shows, as plain values: the games to continue, the library across hosts and the hosts. The
 * screen only maps these to views, so the choices (what is a title, what a host says about itself) are
 * tested here.
 */
public final class HomeModel {
    public enum Badge { NONE, RUNNING, SUSPENDED }

    /** What a host's card says on its second line. */
    public enum HostLine {
        /** "12 games": read from the host just now, or a classic list of an online host. */
        GAMES,
        /** "Offline, 12 saved games": the last library the host sent. */
        SAVED_GAMES,
        OFFLINE,
        /** Online, but this device has no list of its games yet. */
        NO_LIBRARY
    }

    /** A game on a host: a card under Continue playing or in the Library. */
    public static final class GameCard {
        /** Stable across refreshes; the same game on the same host keeps its key in each row. */
        public final String key;
        public final String title;
        public final String hostUuid;
        public final String hostName;
        public final int appId;
        public final Badge badge;
        /** How the host lists the game; null for a recent game the host no longer lists or has no list for. */
        public final ConsoleSnapshot.Source source;
        /** In the library: how many hosts have the game. */
        public final int hostCount;
        /** True when the player starred this game on this host. */
        public final boolean favourite;
        /** A game not added that its host can add on request (library.add); otherwise it is added on the PC. */
        public final boolean addable;
        /** Every PC the card stands for: its own, and in the library all that list the game. */
        public final List<String> hostUuids;

        GameCard(String key, String title, String hostUuid, String hostName, int appId, Badge badge,
                 ConsoleSnapshot.Source source, int hostCount, boolean favourite) {
            this(key, title, hostUuid, hostName, appId, badge, source, hostCount, favourite, false,
                    Collections.singletonList(hostUuid));
        }

        GameCard(String key, String title, String hostUuid, String hostName, int appId, Badge badge,
                 ConsoleSnapshot.Source source, int hostCount, boolean favourite, boolean addable) {
            this(key, title, hostUuid, hostName, appId, badge, source, hostCount, favourite, addable,
                    Collections.singletonList(hostUuid));
        }

        GameCard(String key, String title, String hostUuid, String hostName, int appId, Badge badge,
                 ConsoleSnapshot.Source source, int hostCount, boolean favourite, List<String> hostUuids) {
            this(key, title, hostUuid, hostName, appId, badge, source, hostCount, favourite, false, hostUuids);
        }

        GameCard(String key, String title, String hostUuid, String hostName, int appId, Badge badge,
                 ConsoleSnapshot.Source source, int hostCount, boolean favourite, boolean addable,
                 List<String> hostUuids) {
            this.key = key;
            this.title = title;
            this.hostUuid = hostUuid;
            this.hostName = hostName;
            this.appId = appId;
            this.badge = badge;
            this.source = source;
            this.hostCount = hostCount;
            this.favourite = favourite;
            this.addable = addable;
            this.hostUuids = hostUuids;
        }

        /** The store's key ("steam", ...); null for an app no store owns and for a classic app. */
        public String store() {
            return source == null ? null : source.storeSource;
        }

        /** The store's name as the host writes it, else the one this app knows; null without a store. */
        public String storeName() {
            return source == null ? null : Stores.name(source.storeSource, source.storeName);
        }

        /** The host lists it as not installed: it shows, but cannot start until it is. */
        public boolean notInstalled() {
            return source != null && Boolean.FALSE.equals(source.installed);
        }

        /** The host found it in a store but has not added it; it has no app id to start. */
        public boolean notAdded() {
            return source != null && !source.added;
        }
    }

    /** Which of the library's rows: one per store, the apps of no store, the host's last played, the games not added. */
    public enum RowKind { RECENT, STORE, OTHER, UNADDED }

    /** A library row on TV Home: at most {@link #ROW_CAP} cards, then an "All N" tile when there are more. */
    public static final class Row {
        /** "shelf:recent", "shelf:store:steam", "shelf:other" ("shelf:library" when no game has a store), "shelf:unadded". */
        public final String key;
        public final RowKind kind;
        /** The store's key of a STORE row, else null. */
        public final String store;
        /** The store's name of a STORE row, else null; the other rows' titles are strings of this app. */
        public final String storeName;
        public final List<GameCard> cards;
        /** Every card the row has before the cap; above the cap the row ends in an "All N" tile. */
        public final int total;
        /** The All games filter the "All N" tile opens. */
        public final String filter;

        Row(String key, RowKind kind, String store, String storeName, List<GameCard> all, String filter, boolean capped) {
            this.key = key;
            this.kind = kind;
            this.store = store;
            this.storeName = storeName;
            this.total = all.size();
            this.cards = Collections.unmodifiableList(capped && all.size() > ROW_CAP
                    ? new ArrayList<>(all.subList(0, ROW_CAP)) : all);
            this.filter = filter;
        }

        /** True when the row ends in an "All N" tile. */
        public boolean hasMore() {
            return total > cards.size();
        }
    }

    /** One chip above the All games grid. */
    public static final class Filter {
        /** {@link #FILTER_ALL}, {@link #FILTER_INSTALLED}, "store:steam", {@link #FILTER_OTHER} or {@link #FILTER_UNADDED}. */
        public final String key;
        /** The store's name of a store chip, else null. */
        public final String storeName;
        public final int count;

        Filter(String key, String storeName, int count) {
            this.key = key;
            this.storeName = storeName;
            this.count = count;
        }
    }

    public static final int ROW_CAP = 20;
    public static final String FILTER_ALL = "all";
    public static final String FILTER_INSTALLED = "installed";
    public static final String FILTER_OTHER = "other";
    public static final String FILTER_UNADDED = "unadded";
    static final String STORE_FILTER = "store:";

    /** Whether a card of {@link #allGames} shows under a filter chip. An unknown filter shows everything added. */
    public static boolean matches(String filter, GameCard card) {
        if (FILTER_UNADDED.equals(filter)) return card.notAdded();
        if (card.notAdded()) return false;
        if (filter == null || FILTER_ALL.equals(filter)) return true;
        if (FILTER_INSTALLED.equals(filter)) return !card.notInstalled();
        if (FILTER_OTHER.equals(filter)) return card.store() == null;
        if (filter.startsWith(STORE_FILTER)) return filter.substring(STORE_FILTER.length()).equals(card.store());
        return true;
    }

    public static final class HostCard {
        public final String uuid;
        public final String name;
        public final HostLine line;
        public final int gameCount;
        /** True when the host answers now; a tap opens its library, otherwise the list of hosts. */
        public final boolean online;

        HostCard(String uuid, String name, HostLine line, int gameCount, boolean online) {
            this.uuid = uuid;
            this.name = name;
            this.line = line;
            this.gameCount = gameCount;
            this.online = online;
        }
    }

    /** The starred games, newest star first: the first row of Home. */
    public final List<GameCard> favourites;
    public final List<GameCard> continuePlaying;
    /** The installed games across hosts, by name: the phone's grid. */
    public final List<GameCard> library;
    public final List<HostCard> hosts;
    /** TV Home's library rows, between Favourites and the PCs: Recently played, one per store, Other apps, not added. */
    public final List<Row> rows;
    /** Every game the All games screen can show, by name; {@link #matches} picks a chip's. */
    public final List<GameCard> allGames;
    /** The chips above All games, for what the library has: All, Installed, the stores, Other apps, Not added. */
    public final List<Filter> filters;

    private HomeModel(List<GameCard> favourites, List<GameCard> continuePlaying, List<GameCard> library,
                      List<HostCard> hosts, List<Row> rows, List<GameCard> allGames, List<Filter> filters) {
        this.favourites = Collections.unmodifiableList(favourites);
        this.continuePlaying = Collections.unmodifiableList(continuePlaying);
        this.library = Collections.unmodifiableList(library);
        this.hosts = Collections.unmodifiableList(hosts);
        this.rows = Collections.unmodifiableList(rows);
        this.allGames = Collections.unmodifiableList(allGames);
        this.filters = Collections.unmodifiableList(filters);
    }

    public static HomeModel empty() {
        return new HomeModel(new ArrayList<>(), new ArrayList<>(), new ArrayList<>(), new ArrayList<>(),
                new ArrayList<>(), new ArrayList<>(), new ArrayList<>());
    }

    /** The UUIDs of the hosts that do not let this device start games (no Launch permission). */
    public static Set<String> launchDenied(List<ComputerDetails> hosts) {
        Set<String> denied = new HashSet<>();
        for (ComputerDetails host : hosts) {
            if (!host.hasPermission(ComputerDetails.PERMISSION_LAUNCH)) denied.add(host.uuid);
        }
        return denied;
    }

    /** The row with this key, or null. */
    public Row row(String key) {
        for (Row row : rows) {
            if (row.key.equals(key)) return row;
        }
        return null;
    }

    public boolean isEmpty() {
        return favourites.isEmpty() && continuePlaying.isEmpty() && library.isEmpty() && hosts.isEmpty()
                && rows.isEmpty();
    }

    /** {@code onlineHosts} are the UUIDs of the hosts that answer now. */
    public static HomeModel of(ConsoleSnapshot snapshot, Set<String> onlineHosts) {
        return of(snapshot, onlineHosts, Collections.emptySet());
    }

    /**
     * {@code onlineHosts} are the UUIDs of the hosts that answer now, {@code launchDenied} those where this device
     * may not start games ({@link #launchDenied}): a game they have not added is added on the PC, not from here.
     */
    public static HomeModel of(ConsoleSnapshot snapshot, Set<String> onlineHosts, Set<String> launchDenied) {
        Set<String> starred = new HashSet<>();
        List<GameCard> favourites = new ArrayList<>();
        for (ConsoleSnapshot.Starred entry : snapshot.favourites) {
            starred.add(entry.hostUuid + ":" + entry.appId);
            // A title the host lists as not installed cannot be started from here, as in the library.
            if (entry.source != null && Boolean.FALSE.equals(entry.source.installed)) {
                continue;
            }
            String id = entry.source != null && !entry.source.stableId().isEmpty() ? entry.source.stableId()
                    : String.valueOf(entry.appId);
            favourites.add(new GameCard("favourite:" + entry.hostUuid + ":" + id, entry.title,
                    entry.hostUuid, entry.hostName, entry.appId,
                    entry.source == null ? Badge.NONE : badgeOf(snapshot, entry.source), entry.source, 1, true));
        }
        List<GameCard> continuePlaying = new ArrayList<>();
        for (ConsoleSnapshot.Continue entry : snapshot.continuePlaying) {
            Badge badge = entry.kind == ConsoleSnapshot.ContinueKind.RUNNING ? Badge.RUNNING
                    : entry.kind == ConsoleSnapshot.ContinueKind.SUSPENDED ? Badge.SUSPENDED : Badge.NONE;
            continuePlaying.add(new GameCard("continue:" + entry.hostUuid + ":" + entry.appId, entry.title,
                    entry.hostUuid, entry.hostName, entry.appId, badge, entry.source, 1,
                    starred.contains(entry.hostUuid + ":" + entry.appId)));
        }
        List<GameCard> library = new ArrayList<>();
        for (ConsoleSnapshot.Title title : snapshot.titles) {
            ConsoleSnapshot.Source best = null;
            int hosts = 0;
            List<String> listing = new ArrayList<>();
            for (ConsoleSnapshot.Source source : title.sources) {
                // A library entry that is not installed, or not added, cannot be started from here.
                if (Boolean.FALSE.equals(source.installed) || !source.added) {
                    continue;
                }
                if (best == null) {
                    best = source;
                }
                hosts++;
                listing.add(source.hostUuid);
            }
            if (best == null) {
                continue;
            }
            library.add(new GameCard("game:" + title.key, title.title, best.hostUuid, best.hostName, best.appId,
                    badgeOf(snapshot, best), best, hosts, starred.contains(best.hostUuid + ":" + best.appId), listing));
        }
        List<GameCard> allGames = new ArrayList<>();
        List<Row> rows = libraryRows(snapshot, starred, allGames, launchDenied);
        List<HostCard> hosts = new ArrayList<>();
        for (ConsoleSnapshot.Host host : snapshot.hosts) {
            boolean online = onlineHosts.contains(host.uuid);
            int count = 0;
            for (ConsoleSnapshot.Source source : host.sources) {
                if (source.added) count++;
            }
            HostLine line;
            switch (host.status) {
                case LIVE:
                    line = HostLine.GAMES;
                    break;
                case CLASSIC:
                    line = online ? HostLine.GAMES : HostLine.OFFLINE;
                    break;
                case CACHED:
                    line = online && host.error == null ? HostLine.GAMES : HostLine.SAVED_GAMES;
                    break;
                default:
                    line = online ? HostLine.NO_LIBRARY : HostLine.OFFLINE;
                    break;
            }
            hosts.add(new HostCard(host.uuid, host.name, line, count, online));
        }
        return new HomeModel(favourites, continuePlaying, library, hosts, rows, allGames, filtersOf(allGames));
    }

    /**
     * The library's rows. A title is shown by its best copy: an installed one, else one not installed (it shows
     * desaturated), else, when its host lists the games it has not added, one not added. Each store's row has the
     * installed games first and is as long as the store's share, largest store first; the apps of no store follow.
     */
    private static List<Row> libraryRows(ConsoleSnapshot snapshot, Set<String> starred, List<GameCard> allGames,
                                         Set<String> launchDenied) {
        Map<String, List<GameCard>> byStore = new LinkedHashMap<>();
        Map<String, String> storeNames = new HashMap<>();
        List<GameCard> other = new ArrayList<>();
        List<GameCard> unadded = new ArrayList<>();
        List<GameCard> played = new ArrayList<>();
        for (ConsoleSnapshot.Title title : snapshot.titles) {
            ConsoleSnapshot.Source installed = null;
            ConsoleSnapshot.Source missing = null;
            ConsoleSnapshot.Source found = null;
            boolean addable = false;
            int hosts = 0;
            List<String> listing = new ArrayList<>();
            for (ConsoleSnapshot.Source source : title.sources) {
                if (!source.added) {
                    ConsoleSnapshot.Host host = snapshot.host(source.hostUuid);
                    if (found == null && host != null && host.has(NativeCapabilities.LIBRARY_UNADDED)) {
                        found = source;
                        addable = host.has(NativeCapabilities.LIBRARY_ADD) && !launchDenied.contains(host.uuid);
                    }
                    continue;
                }
                hosts++;
                listing.add(source.hostUuid);
                if (Boolean.FALSE.equals(source.installed)) {
                    if (missing == null) missing = source;
                } else if (installed == null) {
                    installed = source;
                }
            }
            ConsoleSnapshot.Source best = installed != null ? installed : missing;
            if (best == null) {
                if (found != null) {
                    GameCard card = new GameCard("unadded:" + title.key, title.title, found.hostUuid, found.hostName,
                            found.appId, Badge.NONE, found, 1, false, addable);
                    unadded.add(card);
                    allGames.add(card);
                }
                continue;
            }
            boolean star = starred.contains(best.hostUuid + ":" + best.appId);
            GameCard card = new GameCard("game:" + title.key, title.title, best.hostUuid, best.hostName, best.appId,
                    badgeOf(snapshot, best), best, hosts, star, listing);
            allGames.add(card);
            if (best.storeSource == null) {
                other.add(card);
            } else {
                List<GameCard> row = byStore.get(best.storeSource);
                if (row == null) {
                    row = new ArrayList<>();
                    byStore.put(best.storeSource, row);
                }
                row.add(card);
                if (!storeNames.containsKey(best.storeSource) && best.storeName != null) {
                    storeNames.put(best.storeSource, best.storeName);
                }
            }
            Long last = lastPlayed(title);
            if (last != null) {
                played.add(new GameCard("recent:" + title.key, title.title, best.hostUuid, best.hostName, best.appId,
                        card.badge, best, hosts, star, listing));
            }
        }
        List<Row> rows = new ArrayList<>();
        if (!played.isEmpty()) {
            Map<String, Long> at = new HashMap<>();
            for (ConsoleSnapshot.Title title : snapshot.titles) {
                Long last = lastPlayed(title);
                if (last != null) at.put("recent:" + title.key, last);
            }
            // Newest first; the sort is stable, so a tie keeps the order of the names.
            Collections.sort(played, (a, b) -> Long.compare(at.get(b.key), at.get(a.key)));
            if (played.size() > ROW_CAP) played = new ArrayList<>(played.subList(0, ROW_CAP));
            rows.add(new Row("shelf:recent", RowKind.RECENT, null, null, played, FILTER_ALL, false));
        }
        List<String> stores = new ArrayList<>(byStore.keySet());
        Collections.sort(stores, (a, b) -> {
            int bySize = Integer.compare(byStore.get(b).size(), byStore.get(a).size());
            return bySize != 0 ? bySize : a.compareTo(b);
        });
        for (String store : stores) {
            List<GameCard> cards = installedFirst(byStore.get(store));
            rows.add(new Row("shelf:store:" + store, RowKind.STORE, store, Stores.name(store, storeNames.get(store)),
                    cards, STORE_FILTER + store, true));
        }
        if (!other.isEmpty()) {
            // A library with no stores at all (a classic host) is one plain row, as before stores were known.
            boolean plain = stores.isEmpty();
            rows.add(new Row(plain ? "shelf:library" : "shelf:other", RowKind.OTHER, null, null, installedFirst(other),
                    plain ? FILTER_ALL : FILTER_OTHER, true));
        }
        if (!unadded.isEmpty()) {
            rows.add(new Row("shelf:unadded", RowKind.UNADDED, null, null, unadded, FILTER_UNADDED, true));
        }
        return rows;
    }

    /** The newest last-played time of a title's added copies, or null when no host says. */
    private static Long lastPlayed(ConsoleSnapshot.Title title) {
        Long newest = null;
        for (ConsoleSnapshot.Source source : title.sources) {
            if (source.added && source.lastPlayed != null && (newest == null || source.lastPlayed > newest)) {
                newest = source.lastPlayed;
            }
        }
        return newest;
    }

    private static List<GameCard> installedFirst(List<GameCard> cards) {
        List<GameCard> sorted = new ArrayList<>(cards);
        // Stable: the names stay in order within each part.
        Collections.sort(sorted, (a, b) -> Boolean.compare(a.notInstalled(), b.notInstalled()));
        return sorted;
    }

    private static List<Filter> filtersOf(List<GameCard> allGames) {
        int added = 0;
        int installed = 0;
        int other = 0;
        int unadded = 0;
        Map<String, Integer> stores = new LinkedHashMap<>();
        Map<String, String> names = new HashMap<>();
        for (GameCard card : allGames) {
            if (card.notAdded()) {
                unadded++;
                continue;
            }
            added++;
            if (!card.notInstalled()) installed++;
            String store = card.store();
            if (store == null) {
                other++;
            } else {
                Integer count = stores.get(store);
                stores.put(store, count == null ? 1 : count + 1);
                if (!names.containsKey(store)) names.put(store, card.storeName());
            }
        }
        List<Filter> filters = new ArrayList<>();
        if (added == 0 && unadded == 0) return filters;
        filters.add(new Filter(FILTER_ALL, null, added));
        // Installed narrows only when something is not.
        if (installed < added) filters.add(new Filter(FILTER_INSTALLED, null, installed));
        List<String> order = new ArrayList<>(stores.keySet());
        Collections.sort(order, (a, b) -> {
            int bySize = Integer.compare(stores.get(b), stores.get(a));
            return bySize != 0 ? bySize : a.compareTo(b);
        });
        for (String store : order) {
            filters.add(new Filter(STORE_FILTER + store, names.get(store), stores.get(store)));
        }
        if (other > 0 && !stores.isEmpty()) filters.add(new Filter(FILTER_OTHER, null, other));
        if (unadded > 0) filters.add(new Filter(FILTER_UNADDED, null, unadded));
        return filters;
    }

    private static Badge badgeOf(ConsoleSnapshot snapshot, ConsoleSnapshot.Source source) {
        ConsoleSnapshot.Host host = snapshot.host(source.hostUuid);
        NativeSession session = host == null ? null : host.session;
        if (session != null && session.isActive() && session.app.appId == source.appId) {
            return session.state == NativeSession.State.SUSPENDED ? Badge.SUSPENDED : Badge.RUNNING;
        }
        return source.running ? Badge.RUNNING : Badge.NONE;
    }
}

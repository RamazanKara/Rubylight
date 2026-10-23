package com.limelight.console;

import com.limelight.nativeapi.ConsoleSnapshot;
import com.limelight.nativeapi.NativeSession;

import java.util.ArrayList;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;
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

        GameCard(String key, String title, String hostUuid, String hostName, int appId, Badge badge,
                 ConsoleSnapshot.Source source, int hostCount, boolean favourite) {
            this.key = key;
            this.title = title;
            this.hostUuid = hostUuid;
            this.hostName = hostName;
            this.appId = appId;
            this.badge = badge;
            this.source = source;
            this.hostCount = hostCount;
            this.favourite = favourite;
        }
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
    public final List<GameCard> library;
    public final List<HostCard> hosts;

    private HomeModel(List<GameCard> favourites, List<GameCard> continuePlaying, List<GameCard> library,
                      List<HostCard> hosts) {
        this.favourites = Collections.unmodifiableList(favourites);
        this.continuePlaying = Collections.unmodifiableList(continuePlaying);
        this.library = Collections.unmodifiableList(library);
        this.hosts = Collections.unmodifiableList(hosts);
    }

    public static HomeModel empty() {
        return new HomeModel(new ArrayList<>(), new ArrayList<>(), new ArrayList<>(), new ArrayList<>());
    }

    public boolean isEmpty() {
        return favourites.isEmpty() && continuePlaying.isEmpty() && library.isEmpty() && hosts.isEmpty();
    }

    /** {@code onlineHosts} are the UUIDs of the hosts that answer now. */
    public static HomeModel of(ConsoleSnapshot snapshot, Set<String> onlineHosts) {
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
            for (ConsoleSnapshot.Source source : title.sources) {
                // A library entry that is not installed cannot be started from here.
                if (Boolean.FALSE.equals(source.installed)) {
                    continue;
                }
                if (best == null) {
                    best = source;
                }
                hosts++;
            }
            if (best == null) {
                continue;
            }
            library.add(new GameCard("game:" + title.key, title.title, best.hostUuid, best.hostName, best.appId,
                    badgeOf(snapshot, best), best, hosts, starred.contains(best.hostUuid + ":" + best.appId)));
        }
        List<HostCard> hosts = new ArrayList<>();
        for (ConsoleSnapshot.Host host : snapshot.hosts) {
            boolean online = onlineHosts.contains(host.uuid);
            int count = host.sources.size();
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
        return new HomeModel(favourites, continuePlaying, library, hosts);
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

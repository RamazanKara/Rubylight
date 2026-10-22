package com.limelight.nativeapi;

import java.util.Collections;
import java.util.List;

/** What Home shows: every title of every paired host, and the games to come back to. */
public final class ConsoleSnapshot {
    /** Where a host's titles came from. */
    public enum Status {
        /** Read from the native API just now. */
        LIVE,
        /** The last native API library, because the host is offline or did not answer. */
        CACHED,
        /** The classic Moonlight app list, for a host without the native API. */
        CLASSIC,
        /** Nothing to show: no cache yet, or no app list. */
        UNAVAILABLE
    }

    /** One title on one host. */
    public static final class Source {
        public final String hostUuid;
        public final String hostName;
        public final int appId;
        /** The native API's id of the title on the host; null for a classic app. */
        public final String installId;
        public final String title;
        /** The native API's id of the title across hosts; null when unknown and for classic apps. */
        public final String gameId;
        public final boolean running;
        /** The cover's path on the host and its version; null for a classic app. */
        public final String coverPath;
        public final String coverVersion;
        /** Null when unknown. */
        public final Boolean installed;
        /** The classic app's UUID, empty otherwise. */
        public final String appUuid;
        public final boolean hdrSupported;

        Source(String hostUuid, String hostName, int appId, String installId, String title, String gameId,
               boolean running, String coverPath, String coverVersion, Boolean installed, String appUuid,
               boolean hdrSupported) {
            this.hostUuid = hostUuid;
            this.hostName = hostName;
            this.appId = appId;
            this.installId = installId;
            this.title = title;
            this.gameId = gameId;
            this.running = running;
            this.coverPath = coverPath;
            this.coverVersion = coverVersion;
            this.installed = installed;
            this.appUuid = appUuid;
            this.hdrSupported = hdrSupported;
        }
    }

    /** A title, with every host that has it (hosts agree through the native API's game id). */
    public static final class Title {
        public final String key;
        public final String title;
        /** Running copies first, then in the order of the hosts. */
        public final List<Source> sources;

        Title(String key, String title, List<Source> sources) {
            this.key = key;
            this.title = title;
            this.sources = Collections.unmodifiableList(sources);
        }
    }

    /** One host's part of the snapshot. */
    public static final class Host {
        public final String uuid;
        public final String name;
        public final Status status;
        /** When the native library was read (LIVE: now); 0 for a classic list. */
        public final long fetchedAtMs;
        /** Why a live read failed, when the host shows CACHED or UNAVAILABLE for it; otherwise null. */
        public final NativeApiException error;
        /** The host's session, when it was read; null otherwise. */
        public final NativeSession session;
        public final List<Source> sources;

        Host(String uuid, String name, Status status, long fetchedAtMs, NativeApiException error,
             NativeSession session, List<Source> sources) {
            this.uuid = uuid;
            this.name = name;
            this.status = status;
            this.fetchedAtMs = fetchedAtMs;
            this.error = error;
            this.session = session;
            this.sources = Collections.unmodifiableList(sources);
        }
    }

    public enum ContinueKind {
        /** Streaming or starting on the host now. */
        RUNNING,
        /** Suspended on the host; resuming picks it up. */
        SUSPENDED,
        /** Streamed from this device before. */
        RECENT
    }

    /** A game to come back to. */
    public static final class Continue {
        public final ContinueKind kind;
        public final String hostUuid;
        public final String hostName;
        public final int appId;
        public final String title;
        /** The host's session for RUNNING and SUSPENDED entries of native hosts; otherwise null. */
        public final NativeSession session;
        /** The title's source on the host when its library lists it; otherwise null. */
        public final Source source;
        /** When this device last streamed it, 0 for none. */
        public final long playedAtMs;

        Continue(ContinueKind kind, String hostUuid, String hostName, int appId, String title,
                 NativeSession session, Source source, long playedAtMs) {
            this.kind = kind;
            this.hostUuid = hostUuid;
            this.hostName = hostName;
            this.appId = appId;
            this.title = title;
            this.session = session;
            this.source = source;
            this.playedAtMs = playedAtMs;
        }
    }

    /** Titles by name. */
    public final List<Title> titles;
    public final List<Host> hosts;
    /** Running first, then suspended, then recents, newest first. */
    public final List<Continue> continuePlaying;

    ConsoleSnapshot(List<Title> titles, List<Host> hosts, List<Continue> continuePlaying) {
        this.titles = Collections.unmodifiableList(titles);
        this.hosts = Collections.unmodifiableList(hosts);
        this.continuePlaying = Collections.unmodifiableList(continuePlaying);
    }

    public Host host(String uuid) {
        for (Host host : hosts) {
            if (host.uuid.equals(uuid)) {
                return host;
            }
        }
        return null;
    }
}

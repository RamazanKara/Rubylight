package com.limelight.nativeapi;

/** The last library read from each host, so Home renders while a host is offline. */
public interface LibraryCache {
    /** A stored library: the host's /library JSON and when it was read. */
    final class Entry {
        public final String libraryJson;
        public final long fetchedAtMs;

        public Entry(String libraryJson, long fetchedAtMs) {
            this.libraryJson = libraryJson;
            this.fetchedAtMs = fetchedAtMs;
        }
    }

    /** The stored library of a host, or null. */
    Entry load(String hostUuid);

    void store(String hostUuid, Entry entry);

    void forget(String hostUuid);
}

package com.limelight.nativeapi;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/** The last library read from each host, so Home renders while a host is offline. */
public interface LibraryCache {
    /** A stored library: the host's /library JSON, when it was read, and the features the host advertised then. */
    final class Entry {
        public final String libraryJson;
        public final long fetchedAtMs;
        public final List<String> features;

        public Entry(String libraryJson, long fetchedAtMs) {
            this(libraryJson, fetchedAtMs, Collections.emptyList());
        }

        public Entry(String libraryJson, long fetchedAtMs, List<String> features) {
            this.libraryJson = libraryJson;
            this.fetchedAtMs = fetchedAtMs;
            this.features = Collections.unmodifiableList(new ArrayList<>(features));
        }
    }

    /** The stored library of a host, or null. */
    Entry load(String hostUuid);

    void store(String hostUuid, Entry entry);

    void forget(String hostUuid);
}

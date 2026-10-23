package com.limelight.nativeapi;

import java.util.List;

/**
 * The games the player starred, newest star first. A favourite is one game on one host, so the same title on
 * two PCs is two entries. It is kept by the host's own stable id of the game (the native install id, or a
 * classic app's UUID) when the host gave one, because a host may renumber its apps; the app id is the fallback.
 */
public interface FavouritesStore {
    final class Favourite {
        public final String hostUuid;
        public final String hostName;
        public final int appId;
        public final String title;
        /** The host's stable id of the game; empty when it had none. */
        public final String key;

        public Favourite(String hostUuid, String hostName, int appId, String title) {
            this(hostUuid, hostName, "", appId, title);
        }

        public Favourite(String hostUuid, String hostName, String key, int appId, String title) {
            this.hostUuid = hostUuid;
            this.hostName = hostName;
            this.appId = appId;
            this.title = title;
            this.key = key == null ? "" : key;
        }

        public boolean is(String hostUuid, int appId) {
            return is(hostUuid, "", appId);
        }

        /** The same game: by stable id when both sides have one, otherwise by app id. */
        public boolean is(String hostUuid, String key, int appId) {
            if (!this.hostUuid.equals(hostUuid)) {
                return false;
            }
            if (!this.key.isEmpty() && key != null && !key.isEmpty()) {
                return this.key.equals(key);
            }
            return this.appId == appId;
        }
    }

    /** Newest star first. */
    List<Favourite> load();

    boolean isFavourite(String hostUuid, String key, int appId);

    /** Stars the game (first in the list), or unstars it. Starring a starred game only refreshes its names and ids. */
    void set(Favourite favourite, boolean starred);

    /** Drops everything of a host that was removed. */
    void forgetHost(String hostUuid);
}

package com.limelight.nativeapi;

import java.util.List;

/**
 * The games the player starred, newest star first. A favourite is one game on one host, keyed by the host's
 * UUID and the app id, so the same title on two PCs is two entries.
 */
public interface FavouritesStore {
    final class Favourite {
        public final String hostUuid;
        public final String hostName;
        public final int appId;
        public final String title;

        public Favourite(String hostUuid, String hostName, int appId, String title) {
            this.hostUuid = hostUuid;
            this.hostName = hostName;
            this.appId = appId;
            this.title = title;
        }

        public boolean is(String hostUuid, int appId) {
            return this.hostUuid.equals(hostUuid) && this.appId == appId;
        }
    }

    /** Newest star first. */
    List<Favourite> load();

    boolean isFavourite(String hostUuid, int appId);

    /** Stars the game (first in the list), or unstars it. Starring a starred game only refreshes its names. */
    void set(Favourite favourite, boolean starred);

    /** Drops everything of a host that was removed. */
    void forgetHost(String hostUuid);
}

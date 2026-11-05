package com.limelight.nativeapi;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;

import java.util.List;

/** The games this device streamed last, newest first, for "Continue playing". */
public interface RecentsStore {
    final class Recent {
        public final String hostUuid;
        public final String hostName;
        public final int appId;
        public final String title;
        public final long playedAtMs;
        /** What the host calls the game across id changes; empty when it gave nothing or for an older entry. */
        public final String stableId;

        public Recent(String hostUuid, String hostName, int appId, String title, long playedAtMs) {
            this(hostUuid, hostName, appId, title, playedAtMs, "");
        }

        public Recent(String hostUuid, String hostName, int appId, String title, long playedAtMs, String stableId) {
            this.hostUuid = hostUuid;
            this.hostName = hostName;
            this.appId = appId;
            this.title = title;
            this.playedAtMs = playedAtMs;
            this.stableId = stableId == null ? "" : stableId;
        }

        public static Recent of(ComputerDetails computer, NvApp app, long playedAtMs) {
            return new Recent(computer.uuid, computer.name, app.getAppId(), app.getAppName(), playedAtMs, app.getStableId());
        }
    }

    /** Newest first. */
    List<Recent> load();

    /** Puts the game first, replacing an earlier entry of the same game on the same host. */
    void record(Recent recent);

    /** Drops everything of a host that was removed. */
    void forgetHost(String hostUuid);
}

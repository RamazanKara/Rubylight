package com.limelight.console;

import com.limelight.nativeapi.ConsoleSnapshot;

import java.util.ArrayList;
import java.util.List;
import java.util.Set;

/**
 * The PCs that have a game Home lists once, for the question "which PC?": every host that lists it as installed,
 * the ones that answer now first and each group in the order of the hosts.
 */
final class HostChoice {
    final String hostUuid;
    final String hostName;
    final int appId;
    final boolean online;

    private HostChoice(String hostUuid, String hostName, int appId, boolean online) {
        this.hostUuid = hostUuid;
        this.hostName = hostName;
        this.appId = appId;
        this.online = online;
    }

    /**
     * The library card's key for a card of Home that stands for a title on every PC: a library card, or the TV's
     * Recently played card of the same title. Null for a card of one PC (Continue playing, a favourite, not added).
     */
    static String libraryKeyOf(String cardKey) {
        if (cardKey.startsWith("game:")) return cardKey;
        if (cardKey.startsWith("recent:")) return "game:" + cardKey.substring("recent:".length());
        return null;
    }

    /**
     * @param cardKey the key of the library card ({@code "game:" + title key})
     * @param online  the UUIDs of the hosts that answer now
     * @return the hosts that list the game; empty when the snapshot has no such title
     */
    static List<HostChoice> of(ConsoleSnapshot snapshot, String cardKey, Set<String> online) {
        List<HostChoice> awake = new ArrayList<>();
        List<HostChoice> asleep = new ArrayList<>();
        for (ConsoleSnapshot.Title title : snapshot.titles) {
            if (!cardKey.equals("game:" + title.key)) {
                continue;
            }
            for (ConsoleSnapshot.Source source : title.sources) {
                // A copy the PC found but has not added has no app id to open.
                if (Boolean.FALSE.equals(source.installed) || !source.added) {
                    continue;
                }
                boolean up = online.contains(source.hostUuid);
                (up ? awake : asleep).add(new HostChoice(source.hostUuid, source.hostName, source.appId, up));
            }
        }
        awake.addAll(asleep);
        return awake;
    }
}

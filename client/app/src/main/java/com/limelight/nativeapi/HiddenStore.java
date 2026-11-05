package com.limelight.nativeapi;

import android.content.Context;
import android.content.SharedPreferences;

import com.limelight.AppView;

import java.util.Collections;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Map;
import java.util.Set;

/**
 * The games this device hides, per PC: by app id, the list a PC's app grid shares, and beside it each game's stable id.
 * A host renumbers its app ids (a new cover is enough), so the stable id says which game a hidden id follows. Nothing
 * of it goes to the PC.
 */
public interface HiddenStore {
    /** The app ids hidden on this host, as last stored. */
    Set<Integer> of(String hostUuid);

    /** The stable ids this host's hidden games were last seen with, and their app ids then. */
    Map<String, Integer> ids(String hostUuid);

    void save(String hostUuid, Set<Integer> appIds, Map<String, Integer> ids);

    /** Hides a game or shows it again. A game with no stable id ("") goes by its app id alone. */
    default void set(String hostUuid, int appId, String stableId, boolean hidden) {
        synchronized (this) {
            Set<Integer> appIds = new HashSet<>(of(hostUuid));
            Map<String, Integer> ids = new HashMap<>(ids(hostUuid));
            if (hidden) {
                appIds.add(appId);
                if (!stableId.isEmpty()) ids.put(stableId, appId);
            } else {
                appIds.remove(appId);
                ids.remove(stableId);
            }
            save(hostUuid, appIds, ids);
        }
    }

    /**
     * The app ids hidden on this host now that it lists these games (stable id to app id), after following each hidden
     * game to its new id. The app id list stays what counts for a game whose id has not moved: what the PC's grid
     * shows again stays shown, and what it hid, or what was hidden before stable ids were kept, is learned here.
     */
    default Set<Integer> following(String hostUuid, Map<String, Integer> games) {
        synchronized (this) {
            Set<Integer> before = of(hostUuid);
            Map<String, Integer> known = ids(hostUuid);
            Map<String, Integer> kept = new HashMap<>(known);
            // All moves together, so hosts swapping ids cannot lose a hidden choice.
            Set<Integer> from = new HashSet<>();
            Set<Integer> to = new HashSet<>();
            for (Map.Entry<String, Integer> entry : known.entrySet()) {
                Integer now = games.get(entry.getKey());
                // Not listed now: kept, for when it comes back.
                if (now == null) continue;
                int was = entry.getValue();
                if (now == was) {
                    if (!before.contains(was)) kept.remove(entry.getKey());
                } else if (before.contains(was)) {
                    from.add(was);
                    to.add(now);
                    kept.put(entry.getKey(), now);
                } else if (before.contains(now)) {
                    // The PC's open grid followed it first.
                    kept.put(entry.getKey(), now);
                } else {
                    kept.remove(entry.getKey());
                }
            }
            Set<Integer> appIds = new HashSet<>(before);
            appIds.removeAll(from);
            appIds.addAll(to);
            for (Map.Entry<String, Integer> game : games.entrySet()) {
                if (appIds.contains(game.getValue())) kept.put(game.getKey(), game.getValue());
            }
            if (!appIds.equals(before) || !kept.equals(known)) save(hostUuid, appIds, kept);
            return appIds;
        }
    }

    /** The list a PC's app grid has (AppView), so a game hidden in one is hidden in the other. */
    final class Preferences implements HiddenStore {
        private static final String IDS_SUFFIX = "#ids";

        private final SharedPreferences prefs;

        public Preferences(Context context) {
            prefs = context.getSharedPreferences(AppView.HIDDEN_APPS_PREF_FILENAME, Context.MODE_PRIVATE);
        }

        /** The key of a host's stable ids, beside its app id list under the host's UUID. */
        public static String idsKey(String hostUuid) {
            return hostUuid + IDS_SUFFIX;
        }

        @Override
        public Set<Integer> of(String hostUuid) {
            Set<Integer> ids = new HashSet<>();
            for (String id : prefs.getStringSet(hostUuid, Collections.emptySet())) {
                try {
                    ids.add(Integer.parseInt(id));
                } catch (NumberFormatException e) {
                    // AppView skips what it cannot read in the same list.
                    com.limelight.LimeLog.warning("Hidden games: not an app id: " + id);
                }
            }
            return ids;
        }

        /** Stored as "appId:stableId": a stable id may hold anything, an app id no colon. */
        @Override
        public Map<String, Integer> ids(String hostUuid) {
            Map<String, Integer> ids = new HashMap<>();
            for (String entry : prefs.getStringSet(idsKey(hostUuid), Collections.emptySet())) {
                int at = entry.indexOf(':');
                try {
                    ids.put(entry.substring(at + 1), Integer.parseInt(entry.substring(0, Math.max(at, 0))));
                } catch (NumberFormatException e) {
                    com.limelight.LimeLog.warning("Hidden games: not an id: " + entry);
                }
            }
            return ids;
        }

        @Override
        public synchronized void save(String hostUuid, Set<Integer> appIds, Map<String, Integer> ids) {
            Set<String> list = new HashSet<>();
            for (int id : appIds) list.add(Integer.toString(id));
            Set<String> named = new HashSet<>();
            for (Map.Entry<String, Integer> id : ids.entrySet()) named.add(id.getValue() + ":" + id.getKey());
            prefs.edit().putStringSet(hostUuid, list).putStringSet(idsKey(hostUuid), named).apply();
        }
    }

    /** Kept for as long as the repository lives (tests, the demo). */
    final class InMemory implements HiddenStore {
        private final Map<String, Set<Integer>> hosts = new HashMap<>();
        private final Map<String, Map<String, Integer>> named = new HashMap<>();

        @Override
        public synchronized Set<Integer> of(String hostUuid) {
            Set<Integer> ids = hosts.get(hostUuid);
            return ids == null ? Collections.emptySet() : new HashSet<>(ids);
        }

        @Override
        public synchronized Map<String, Integer> ids(String hostUuid) {
            Map<String, Integer> ids = named.get(hostUuid);
            return ids == null ? Collections.emptyMap() : new HashMap<>(ids);
        }

        @Override
        public synchronized void save(String hostUuid, Set<Integer> appIds, Map<String, Integer> ids) {
            hosts.put(hostUuid, new HashSet<>(appIds));
            named.put(hostUuid, new HashMap<>(ids));
        }
    }
}

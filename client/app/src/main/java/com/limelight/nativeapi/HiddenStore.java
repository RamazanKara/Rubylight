package com.limelight.nativeapi;

import android.content.Context;
import android.content.SharedPreferences;

import com.limelight.AppView;

import java.util.Collections;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Map;
import java.util.Set;

/** The games this device hides, per PC by app id. Nothing of it goes to the PC. */
public interface HiddenStore {
    /** The app ids hidden on this host. */
    Set<Integer> of(String hostUuid);

    void set(String hostUuid, int appId, boolean hidden);

    /** The list a PC's app grid has (AppView), so a game hidden in one is hidden in the other. */
    final class Preferences implements HiddenStore {
        private final SharedPreferences prefs;

        public Preferences(Context context) {
            prefs = context.getSharedPreferences(AppView.HIDDEN_APPS_PREF_FILENAME, Context.MODE_PRIVATE);
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

        @Override
        public synchronized void set(String hostUuid, int appId, boolean hidden) {
            Set<String> ids = new HashSet<>(prefs.getStringSet(hostUuid, Collections.emptySet()));
            if (hidden) {
                ids.add(Integer.toString(appId));
            } else {
                ids.remove(Integer.toString(appId));
            }
            prefs.edit().putStringSet(hostUuid, ids).apply();
        }
    }

    /** Kept for as long as the repository lives (tests, the demo). */
    final class InMemory implements HiddenStore {
        private final Map<String, Set<Integer>> hosts = new HashMap<>();

        @Override
        public synchronized Set<Integer> of(String hostUuid) {
            Set<Integer> ids = hosts.get(hostUuid);
            return ids == null ? Collections.emptySet() : new HashSet<>(ids);
        }

        @Override
        public synchronized void set(String hostUuid, int appId, boolean hidden) {
            Set<Integer> ids = hosts.computeIfAbsent(hostUuid, host -> new HashSet<>());
            if (hidden) {
                ids.add(appId);
            } else {
                ids.remove(appId);
            }
        }
    }
}

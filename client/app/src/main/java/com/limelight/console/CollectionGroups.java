package com.limelight.console;

import androidx.annotation.VisibleForTesting;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/**
 * Home's groups by collection, the same ones desktop Nightjar draws: nightjar-policy's collections
 * (protocol/policy), reached through libnightjar_core (protocol/android/src/collections.rs).
 */
public final class CollectionGroups {
    private CollectionGroups() { }

    /** One JSON request in, one JSON answer out: {@code {"ok": ...}} or {@code {"error": code, "detail": ...}}. */
    public interface Calls {
        String call(String request);
    }

    /** One group: its collection's name, null for the games in none, and the places of its games in the list asked about. */
    public static final class Group {
        public final String name;
        public final int[] games;

        Group(String name, int[] games) {
            this.name = name;
            this.games = games;
        }
    }

    private static Calls calls;

    private static synchronized Calls calls() {
        if (calls == null) {
            System.loadLibrary("nightjar_core");
            calls = CollectionGroups::call;
        }
        return calls;
    }

    /** JVM tests cannot load the library. */
    @VisibleForTesting
    public static synchronized void use(Calls replacement) {
        calls = replacement;
    }

    private static native String call(String request);

    /**
     * The groups of these games' collections by place: a group per collection by name, then the games in none. Empty
     * when no game is in one, without asking, and when the library cannot answer: Home then shows its rows by store.
     */
    static List<Group> of(List<List<String>> games) {
        boolean any = false;
        JSONArray request = new JSONArray();
        for (List<String> names : games) {
            any |= !names.isEmpty();
            request.put(new JSONArray(names));
        }
        if (!any) return Collections.emptyList();
        try {
            JSONObject answer = new JSONObject(calls().call(new JSONObject().put("games", request).toString()));
            JSONObject ok = answer.optJSONObject("ok");
            if (ok == null) throw new JSONException(answer.toString());
            JSONArray groups = ok.getJSONArray("groups");
            List<Group> out = new ArrayList<>(groups.length());
            for (int i = 0; i < groups.length(); i++) {
                JSONObject group = groups.getJSONObject(i);
                JSONArray places = group.getJSONArray("games");
                int[] at = new int[places.length()];
                for (int j = 0; j < at.length; j++) at[j] = places.getInt(j);
                out.add(new Group(group.isNull("name") ? null : group.getString("name"), at));
            }
            return out;
        } catch (JSONException | LinkageError | RuntimeException e) {
            com.limelight.LimeLog.warning("Collections could not be grouped: " + e);
            return Collections.emptyList();
        }
    }
}

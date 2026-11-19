package com.limelight.console;

import androidx.annotation.VisibleForTesting;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.List;
import java.util.Locale;

/**
 * The order Home and All games list a library in, the same one desktop Nightjar sorts with: nightjar-policy's
 * library_order (protocol/policy), reached through libnightjar_core (protocol/android/src/library_order.rs).
 */
public final class LibraryOrder {
    private LibraryOrder() { }

    /** One JSON request in, one JSON answer out: {@code {"ok": ...}} or {@code {"error": code, "detail": ...}}. */
    public interface Calls {
        String call(String request);
    }

    /** What the order reads of one game: its title, and when and how long its PCs say it was played (null: none says). */
    public static final class Facts {
        final String title;
        final Long lastPlayed;
        final Long playtimeMinutes;

        public Facts(String title, Long lastPlayed, Long playtimeMinutes) {
            this.title = title;
            this.lastPlayed = lastPlayed;
            this.playtimeMinutes = playtimeMinutes;
        }
    }

    private static Calls calls;

    private static synchronized Calls calls() {
        if (calls == null) {
            System.loadLibrary("nightjar_core");
            calls = LibraryOrder::call;
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
     * The places of these games in the chosen order. Null when the library cannot answer: the caller keeps the order it
     * has, by title.
     */
    static int[] of(List<Facts> games, HomeView.Sort sort, HomeView.Order order) {
        try {
            JSONArray list = new JSONArray();
            for (Facts game : games) {
                JSONObject facts = new JSONObject().put("title", game.title == null ? "" : game.title);
                if (game.lastPlayed != null) facts.put("last_played", game.lastPlayed.longValue());
                if (game.playtimeMinutes != null) facts.put("playtime_minutes", game.playtimeMinutes.longValue());
                list.put(facts);
            }
            JSONObject request = new JSONObject().put("key", sort.name().toLowerCase(Locale.ROOT))
                    .put("direction", order.name().toLowerCase(Locale.ROOT)).put("games", list);
            JSONObject answer = new JSONObject(calls().call(request.toString()));
            JSONObject ok = answer.optJSONObject("ok");
            if (ok == null) throw new JSONException(answer.toString());
            JSONArray places = ok.getJSONArray("order");
            if (places.length() != games.size()) throw new JSONException("an order of " + places.length() + " for " + games.size());
            int[] at = new int[places.length()];
            for (int i = 0; i < at.length; i++) at[i] = places.getInt(i);
            return at;
        } catch (JSONException | LinkageError | RuntimeException e) {
            com.limelight.LimeLog.warning("The library could not be sorted: " + e);
            return null;
        }
    }
}

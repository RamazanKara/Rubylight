package com.limelight.nativeapi;

import android.content.Context;
import android.content.SharedPreferences;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.List;

/** {@link RecentsStore} in its own preferences file, a small JSON list. */
public final class PreferencesRecentsStore implements RecentsStore {
    static final String PREFS = "console_recents";
    static final String KEY = "recents";
    static final int MAX = 20;

    private final SharedPreferences prefs;

    public PreferencesRecentsStore(Context context) {
        this.prefs = context.getApplicationContext().getSharedPreferences(PREFS, Context.MODE_PRIVATE);
    }

    @Override
    public synchronized List<Recent> load() {
        List<Recent> recents = new ArrayList<>();
        try {
            JSONArray array = new JSONArray(prefs.getString(KEY, "[]"));
            for (int i = 0; i < array.length(); i++) {
                JSONObject json = array.getJSONObject(i);
                recents.add(new Recent(json.getString("host"), json.optString("host_name"), json.getInt("app"),
                        json.optString("title"), json.optLong("played_ms")));
            }
        } catch (JSONException e) {
            recents.clear();
        }
        return recents;
    }

    @Override
    public synchronized void record(Recent recent) {
        List<Recent> recents = load();
        for (int i = recents.size() - 1; i >= 0; i--) {
            if (recents.get(i).hostUuid.equals(recent.hostUuid) && recents.get(i).appId == recent.appId) {
                recents.remove(i);
            }
        }
        recents.add(0, recent);
        save(recents.size() > MAX ? recents.subList(0, MAX) : recents);
    }

    @Override
    public synchronized void forgetHost(String hostUuid) {
        List<Recent> recents = load();
        for (int i = recents.size() - 1; i >= 0; i--) {
            if (recents.get(i).hostUuid.equals(hostUuid)) {
                recents.remove(i);
            }
        }
        save(recents);
    }

    private void save(List<Recent> recents) {
        JSONArray array = new JSONArray();
        try {
            for (Recent recent : recents) {
                array.put(new JSONObject().put("host", recent.hostUuid).put("host_name", recent.hostName)
                        .put("app", recent.appId).put("title", recent.title).put("played_ms", recent.playedAtMs));
            }
        } catch (JSONException e) {
            return;
        }
        prefs.edit().putString(KEY, array.toString()).apply();
    }
}

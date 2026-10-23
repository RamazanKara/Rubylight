package com.limelight.nativeapi;

import android.content.Context;
import android.content.SharedPreferences;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.List;

/**
 * {@link FavouritesStore} in its own preferences file, a small JSON list. It is plain app data, so it
 * stays through an app update.
 */
public final class PreferencesFavouritesStore implements FavouritesStore {
    static final String PREFS = "console_favourites";
    static final String KEY = "favourites";

    private final SharedPreferences prefs;

    public PreferencesFavouritesStore(Context context) {
        this.prefs = context.getApplicationContext().getSharedPreferences(PREFS, Context.MODE_PRIVATE);
    }

    @Override
    public synchronized List<Favourite> load() {
        List<Favourite> favourites = new ArrayList<>();
        try {
            JSONArray array = new JSONArray(prefs.getString(KEY, "[]"));
            for (int i = 0; i < array.length(); i++) {
                JSONObject json = array.getJSONObject(i);
                favourites.add(new Favourite(json.getString("host"), json.optString("host_name"), json.getInt("app"),
                        json.optString("title")));
            }
        } catch (JSONException e) {
            favourites.clear();
        }
        return favourites;
    }

    @Override
    public synchronized boolean isFavourite(String hostUuid, int appId) {
        for (Favourite favourite : load()) {
            if (favourite.is(hostUuid, appId)) {
                return true;
            }
        }
        return false;
    }

    @Override
    public synchronized void set(Favourite favourite, boolean starred) {
        List<Favourite> favourites = load();
        int at = -1;
        for (int i = 0; i < favourites.size(); i++) {
            if (favourites.get(i).is(favourite.hostUuid, favourite.appId)) {
                at = i;
                break;
            }
        }
        if (!starred) {
            if (at >= 0) {
                favourites.remove(at);
                save(favourites);
            }
            return;
        }
        if (at >= 0) {
            favourites.set(at, favourite);
        } else {
            favourites.add(0, favourite);
        }
        save(favourites);
    }

    @Override
    public synchronized void forgetHost(String hostUuid) {
        List<Favourite> favourites = load();
        if (favourites.removeIf(favourite -> favourite.hostUuid.equals(hostUuid))) {
            save(favourites);
        }
    }

    private void save(List<Favourite> favourites) {
        JSONArray array = new JSONArray();
        try {
            for (Favourite favourite : favourites) {
                array.put(new JSONObject().put("host", favourite.hostUuid).put("host_name", favourite.hostName)
                        .put("app", favourite.appId).put("title", favourite.title));
            }
        } catch (JSONException e) {
            return;
        }
        prefs.edit().putString(KEY, array.toString()).apply();
    }
}

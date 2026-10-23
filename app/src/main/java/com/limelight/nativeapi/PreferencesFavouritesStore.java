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
 * stays through an app update. A bad entry is skipped; a list that cannot be read at all is copied to a
 * backup key before the next write replaces it.
 */
public final class PreferencesFavouritesStore implements FavouritesStore {
    static final String PREFS = "console_favourites";
    static final String KEY = "favourites";
    static final String BACKUP_KEY = "favourites.bak";

    /** One lock for every instance: the static forget-host path and the screens each make their own store. */
    private static final Object LOCK = new Object();

    private final SharedPreferences prefs;

    public PreferencesFavouritesStore(Context context) {
        this.prefs = context.getApplicationContext().getSharedPreferences(PREFS, Context.MODE_PRIVATE);
    }

    @Override
    public List<Favourite> load() {
        synchronized (LOCK) {
            return read();
        }
    }

    @Override
    public boolean isFavourite(String hostUuid, String key, int appId) {
        synchronized (LOCK) {
            for (Favourite favourite : read()) {
                if (favourite.is(hostUuid, key, appId)) {
                    return true;
                }
            }
            return false;
        }
    }

    @Override
    public void set(Favourite favourite, boolean starred) {
        synchronized (LOCK) {
            List<Favourite> favourites = read();
            int at = -1;
            for (int i = 0; i < favourites.size(); i++) {
                if (favourites.get(i).is(favourite.hostUuid, favourite.key, favourite.appId)) {
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
    }

    @Override
    public void forgetHost(String hostUuid) {
        synchronized (LOCK) {
            List<Favourite> favourites = read();
            if (favourites.removeIf(favourite -> favourite.hostUuid.equals(hostUuid))) {
                save(favourites);
            }
        }
    }

    private List<Favourite> read() {
        List<Favourite> favourites = new ArrayList<>();
        String raw = prefs.getString(KEY, "[]");
        JSONArray array;
        try {
            array = new JSONArray(raw);
        } catch (JSONException e) {
            // Keep what was there: the next save would overwrite it with an empty list.
            if (!raw.isEmpty() && !raw.equals(prefs.getString(BACKUP_KEY, null))) {
                prefs.edit().putString(BACKUP_KEY, raw).apply();
            }
            return favourites;
        }
        boolean skipped = false;
        for (int i = 0; i < array.length(); i++) {
            try {
                JSONObject json = array.getJSONObject(i);
                favourites.add(new Favourite(json.getString("host"), json.optString("host_name"),
                        json.optString("key"), json.getInt("app"), json.optString("title")));
            } catch (JSONException e) {
                // One bad entry does not cost the others.
                skipped = true;
            }
        }
        if (skipped && !raw.equals(prefs.getString(BACKUP_KEY, null))) {
            prefs.edit().putString(BACKUP_KEY, raw).apply();
        }
        return favourites;
    }

    private void save(List<Favourite> favourites) {
        JSONArray array = new JSONArray();
        try {
            for (Favourite favourite : favourites) {
                array.put(new JSONObject().put("host", favourite.hostUuid).put("host_name", favourite.hostName)
                        .put("key", favourite.key).put("app", favourite.appId).put("title", favourite.title));
            }
        } catch (JSONException e) {
            return;
        }
        prefs.edit().putString(KEY, array.toString()).apply();
    }
}

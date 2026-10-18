package com.limelight.utils;

import android.content.Context;
import android.content.SharedPreferences;
import android.net.Uri;

/** Last successful export destination, kept separately for each PC and launcher. */
public final class FrontendExportStore {
    private final SharedPreferences preferences;
    private final String key;

    public FrontendExportStore(Context context, String hostUuid, FrontendLauncher launcher) {
        preferences = context.getSharedPreferences("frontend_exports", Context.MODE_PRIVATE);
        key = hostUuid + ":" + launcher.name();
    }

    public Uri romsTree() { return read(":roms"); }
    public Uri esdeTree() { return read(":esde"); }

    private Uri read(String suffix) {
        String value = preferences.getString(key + suffix, null);
        return value == null ? null : Uri.parse(value);
    }

    public void save(Uri roms, Uri esde) {
        preferences.edit().putString(key + ":roms", roms.toString())
                .putString(key + ":esde", esde == null ? null : esde.toString()).apply();
    }
}

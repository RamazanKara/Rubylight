package com.limelight.utils;

import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.net.Uri;

import com.limelight.LimeLog;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * Last successful export destination, kept separately for each PC and launcher. A stored export
 * is kept in sync with the PC's games ({@link FrontendSync}) until the user stops it.
 */
public final class FrontendExportStore {
    static final String PREFERENCES = "frontend_exports";
    // Guards check-then-write against "Stop syncing" on the UI thread; never held across I/O.
    private static final Object LOCK = new Object();

    private final Context context;
    private final SharedPreferences preferences;
    private final String key;

    public FrontendExportStore(Context context, String hostUuid, FrontendLauncher launcher) {
        this.context = context.getApplicationContext();
        preferences = context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE);
        key = hostUuid + ":" + launcher.name();
    }

    public Uri romsTree() { return read(":roms"); }
    public Uri esdeTree() { return read(":esde"); }

    /** The app set of the last export, so an unchanged list exports nothing. */
    public String fingerprint() { return preferences.getString(key + ":apps", null); }

    /** Syncing stopped because the folder could not be written; a successful export resumes it. */
    public boolean failed() { return preferences.getBoolean(key + ":failed", false); }

    private Uri read(String suffix) {
        String value = preferences.getString(key + suffix, null);
        return value == null ? null : Uri.parse(value);
    }

    /**
     * Covers Nightjar wrote into this export's ES-DE folder, by file name, with the cover's asset
     * key. Only these are ever replaced or removed; empty when the export goes to another folder.
     */
    public Map<String, String> covers(Uri esde) {
        Map<String, String> covers = new HashMap<>();
        Uri stored = esdeTree();
        if (esde == null || !esde.equals(stored)) return covers;
        String value = preferences.getString(key + ":covers", "");
        for (String line : value.split("\n")) {
            int tab = line.indexOf('\t');
            if (tab > 0) covers.put(line.substring(0, tab), line.substring(tab + 1));
        }
        return covers;
    }

    public void save(Uri roms, Uri esde) {
        save(roms, esde, null, new HashMap<String, String>());
    }

    public void save(Uri roms, Uri esde, String fingerprint, Map<String, String> covers) {
        StringBuilder coverList = new StringBuilder();
        for (Map.Entry<String, String> cover : covers.entrySet()) {
            coverList.append(cover.getKey()).append('\t').append(cover.getValue()).append('\n');
        }
        Uri oldRoms;
        Uri oldEsde;
        synchronized (LOCK) {
            oldRoms = romsTree();
            oldEsde = esdeTree();
            preferences.edit().putString(key + ":roms", roms.toString())
                    .putString(key + ":esde", esde == null ? null : esde.toString())
                    .putString(key + ":apps", fingerprint)
                    .putString(key + ":covers", coverList.toString())
                    .remove(key + ":failed").apply();
        }
        releaseUnused(oldRoms);
        releaseUnused(oldEsde);
    }

    /** Saves a background sync's result unless the user stopped syncing or moved the export meanwhile. */
    boolean saveIfUnchanged(Uri roms, Uri esde, String fingerprint, Map<String, String> covers) {
        synchronized (LOCK) {
            if (!sameFolders(roms, esde) || failed()) {
                return false;
            }
            save(roms, esde, fingerprint, covers);
            return true;
        }
    }

    /**
     * Stops syncing this export after its folders failed. False when the user stopped syncing or
     * picked other folders meanwhile: that export is not marked, and nothing is to be reported.
     */
    public boolean markFailed(Uri roms, Uri esde) {
        synchronized (LOCK) {
            if (!sameFolders(roms, esde)) return false;
            preferences.edit().putBoolean(key + ":failed", true).apply();
            return true;
        }
    }

    private boolean sameFolders(Uri roms, Uri esde) {
        return roms.equals(romsTree()) && (esde == null ? esdeTree() == null : esde.equals(esdeTree()));
    }

    /** Stops syncing: the destination is forgotten and the files stay where they are. */
    public void forget() {
        Uri roms = romsTree();
        Uri esde = esdeTree();
        synchronized (LOCK) {
            preferences.edit().remove(key + ":roms").remove(key + ":esde").remove(key + ":apps")
                    .remove(key + ":covers").remove(key + ":failed").apply();
        }
        releaseUnused(roms);
        releaseUnused(esde);
    }

    /** Launchers this PC was exported to, in menu order. */
    public static List<FrontendLauncher> exported(Context context, String hostUuid) {
        List<FrontendLauncher> launchers = new ArrayList<>();
        for (FrontendLauncher launcher : FrontendLauncher.values()) {
            if (new FrontendExportStore(context, hostUuid, launcher).romsTree() != null) launchers.add(launcher);
        }
        return launchers;
    }

    /** The PC was removed: stop syncing all of its exports. */
    public static void forgetHost(Context context, String hostUuid) {
        for (FrontendLauncher launcher : exported(context, hostUuid)) {
            new FrontendExportStore(context, hostUuid, launcher).forget();
        }
    }

    /** Persisted folder grants are limited; give one back once no export uses it. */
    private void releaseUnused(Uri tree) {
        if (tree == null || preferences.getAll().containsValue(tree.toString())) return;
        try {
            context.getContentResolver().releasePersistableUriPermission(tree,
                    Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION);
        } catch (SecurityException e) {
            // Never persisted (the provider only granted that visit): nothing to give back.
            LimeLog.info("Frontend folder had no persisted access to release");
        }
    }
}

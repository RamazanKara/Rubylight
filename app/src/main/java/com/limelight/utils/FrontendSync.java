package com.limelight.utils;

import android.content.Context;
import android.content.UriPermission;
import android.net.Uri;
import android.os.Handler;
import android.os.Looper;
import android.widget.Toast;

import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.computers.IdentityManager;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

/**
 * Keeps each launcher export in sync with its PC: when the PC's games or the hidden ones change,
 * the export is written again a moment later, on its own low-priority thread, and games that are
 * gone or hidden leave the launcher. A folder that can no longer be written stops syncing loudly.
 */
public final class FrontendSync {
    static final long DELAY_MS = 3000;
    /** Held by every export so a manual one and a sync never write the same folder at once. */
    public static final Object EXPORT_LOCK = new Object();

    private static final ScheduledExecutorService EXECUTOR = Executors.newSingleThreadScheduledExecutor(r -> {
        Thread thread = new Thread(r, "Frontend sync");
        thread.setDaemon(true);
        thread.setPriority(Thread.MIN_PRIORITY);
        return thread;
    });
    private static final Map<String, ScheduledFuture<?>> PENDING = new HashMap<>();

    private FrontendSync() {}

    /** The PC's app list was fetched or its hidden games changed; cheap when it has no export. */
    public static void appListChanged(Context context, final ComputerDetails computer) {
        if (computer == null || computer.uuid == null) return;
        final Context app = context.getApplicationContext();
        if (FrontendExportStore.exported(app, computer.uuid).isEmpty()) return;
        final String uuid = computer.uuid;
        synchronized (PENDING) {
            ScheduledFuture<?> previous = PENDING.remove(uuid);
            if (previous != null) previous.cancel(false);
            PENDING.put(uuid, EXECUTOR.schedule(() -> {
                synchronized (PENDING) {
                    PENDING.remove(uuid);
                }
                try {
                    FrontendExporter exporter = new FrontendExporter(app, computer);
                    exporter.downloadMissingCovers(new IdentityManager(app).getUniqueId());
                    List<FrontendLauncher> failed = sync(app, computer, exporter);
                    if (!failed.isEmpty()) announce(app, computer.name, failed);
                } catch (RuntimeException e) {
                    // A scheduled task's exception would vanish into its future.
                    LimeLog.warning("Frontend sync failed: " + e);
                }
            }, DELAY_MS, TimeUnit.MILLISECONDS));
        }
    }

    /**
     * Writes every export of this PC whose games changed since its last export. Returns the
     * launchers whose folder failed; they stop syncing until an export by hand succeeds.
     */
    static List<FrontendLauncher> sync(Context context, ComputerDetails computer, FrontendExporter exporter) {
        List<FrontendLauncher> failed = new ArrayList<>();
        List<NvApp> apps = null;
        String fingerprint = null;
        for (FrontendLauncher launcher : FrontendExportStore.exported(context, computer.uuid)) {
            FrontendExportStore store = new FrontendExportStore(context, computer.uuid, launcher);
            Uri roms = store.romsTree();
            Uri esde = store.esdeTree();
            if (roms == null || store.failed()) continue;
            if (apps == null) {
                apps = exporter.loadApps();
                // No cached list (or every game hidden) is no reason to empty the launcher.
                if (apps.isEmpty()) return failed;
                fingerprint = fingerprint(computer.name, apps);
            }
            if (fingerprint.equals(store.fingerprint())) continue;
            try {
                if (!hasAccess(context, roms) || (esde != null && !hasAccess(context, esde))) {
                    throw new IOException("No lasting access to the folder");
                }
                Map<String, String> covers = store.covers(esde);
                synchronized (EXPORT_LOCK) {
                    exporter.export(apps, roms, esde, covers);
                }
                store.saveIfUnchanged(roms, esde, fingerprint, covers);
                LimeLog.info("Frontend sync wrote " + apps.size() + " games for " + launcher.label);
            } catch (IOException | RuntimeException e) {
                LimeLog.warning("Frontend sync for " + launcher.label + " stopped: " + e);
                store.markFailed(roms);
                failed.add(launcher);
            }
        }
        return failed;
    }

    /** What an export writes: the PC's name and each shown game's identity, name and art. */
    public static String fingerprint(String hostName, List<NvApp> apps) {
        List<String> lines = new ArrayList<>();
        for (NvApp app : apps) {
            lines.add(app.getAppId() + "\t" + app.getAppUuid() + "\t" + app.getAppName() + "\t" + app.getAssetCacheKey());
        }
        Collections.sort(lines);
        StringBuilder all = new StringBuilder(String.valueOf(hostName));
        for (String line : lines) all.append('\n').append(line);
        try {
            byte[] digest = MessageDigest.getInstance("SHA-256").digest(all.toString().getBytes(StandardCharsets.UTF_8));
            StringBuilder hex = new StringBuilder();
            for (byte b : digest) hex.append(String.format("%02x", b));
            return hex.toString();
        } catch (NoSuchAlgorithmException e) {
            throw new IllegalStateException(e);
        }
    }

    /** A folder picked once stays writable only with a persisted grant. */
    static boolean hasAccess(Context context, Uri tree) {
        for (UriPermission permission : context.getContentResolver().getPersistedUriPermissions()) {
            if (permission.getUri().equals(tree) && permission.isReadPermission() && permission.isWritePermission()) {
                return true;
            }
        }
        return false;
    }

    private static void announce(final Context context, final String hostName, List<FrontendLauncher> failed) {
        StringBuilder labels = new StringBuilder();
        for (FrontendLauncher launcher : failed) {
            if (labels.length() > 0) labels.append(", ");
            labels.append(launcher.label);
        }
        final String message = context.getString(R.string.frontend_sync_failed, labels, hostName);
        new Handler(Looper.getMainLooper()).post(() -> Toast.makeText(context, message, Toast.LENGTH_LONG).show());
    }
}

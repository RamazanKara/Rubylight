package com.limelight.update;

import android.app.Activity;
import android.content.Context;
import android.content.SharedPreferences;

import com.limelight.LimeLog;
import com.limelight.nativeapi.KnownUpdates;
import com.limelight.nvstream.http.HostCapabilities;

public final class HostUpdateChecker {
    private static final String LATEST_RELEASE_URL = "https://api.github.com/repos/RamazanKara/Rubylight/releases/latest";

    private HostUpdateChecker() { }

    private static SharedPreferences state(Context context) {
        return context.getSharedPreferences("host_update_state", Context.MODE_PRIVATE);
    }

    /**
     * Only a build that updates itself asks GitHub, and there Settings can turn the check off. A store build hides that
     * switch, so it never asks: it shows what the host itself reports ({@link KnownUpdates}).
     */
    private static boolean isDue(Context context, HostCapabilities caps) {
        return UpdateFlow.isAvailable(context) &&
                caps.family() == HostCapabilities.Family.RUBYLIGHT && SemVer.parse(caps.hostVersion()) != null &&
                UpdatePolicy.isCheckDue(new UpdatePrefs(context).isAutoCheck(),
                        state(context).getLong("last_check", 0), System.currentTimeMillis());
    }

    /**
     * The newer Rubylight version for this PC, or null. The host's own answer (GET /update, which also knows its
     * update channel) wins; without one, a build that updates itself falls back to the latest GitHub release.
     */
    public static String availableVersion(Context context, String hostUuid, HostCapabilities caps) {
        if (caps.family() != HostCapabilities.Family.RUBYLIGHT) return null;
        if (KnownUpdates.answered(hostUuid)) return KnownUpdates.offered(hostUuid);
        if (!UpdateFlow.isAvailable(context)) return null;
        String latest = state(context).getString("latest_version", "");
        return SemVer.isNewer(latest, caps.hostVersion()) ? latest : null;
    }

    public static void check(Activity activity, HostCapabilities caps, Runnable changed) {
        if (!isDue(activity, caps)) return;
        new Thread(() -> {
            refresh(activity.getApplicationContext(), caps);
            activity.runOnUiThread(() -> {
                if (!activity.isFinishing() && !activity.isDestroyed()) changed.run();
            });
        }, "nightjar-host-update-check").start();
    }

    // PC polling and direct stream launches share one cached release, regardless of the number of PCs.
    static synchronized void refresh(Context context, HostCapabilities caps) {
        if (!isDue(context, caps)) return;
        SharedPreferences prefs = state(context);
        prefs.edit().putLong("last_check", System.currentTimeMillis()).apply();
        try {
            ReleaseInfo release = UpdateChecker.fetchLatest(LATEST_RELEASE_URL);
            prefs.edit().putString("latest_version", release.isUpdateFor("0.0.0") ? release.version() : "").apply();
        } catch (Exception e) {
            LimeLog.info("Host update check failed: " + e);
        }
    }
}

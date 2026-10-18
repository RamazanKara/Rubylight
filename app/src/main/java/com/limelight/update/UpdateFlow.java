package com.limelight.update;

import android.app.Activity;
import android.content.Context;
import android.text.format.Formatter;
import android.widget.LinearLayout;
import android.widget.TextView;
import android.widget.Toast;

import androidx.appcompat.app.AlertDialog;

import com.limelight.ui.NavigationDialogBuilder;
import com.google.android.material.progressindicator.LinearProgressIndicator;
import com.limelight.BuildConfig;
import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.utils.UiHelper;

import java.io.File;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.function.BooleanSupplier;

/** The update screens: the periodic check, the offer, the download progress and the install hand-off. */
public final class UpdateFlow {
    private static final int NOTES_MAX_CHARS = 1200;

    private static final AtomicBoolean checking = new AtomicBoolean();
    private static final AtomicBoolean updating = new AtomicBoolean();

    private UpdateFlow() {
    }

    /** Whether the updater exists in this build and install: not from Google Play, not built for a store. */
    public static boolean isAvailable(Context context) {
        return UpdateInstaller.isAvailable(context);
    }

    /**
     * Looks for an update when the automatic check is on and none ran in the last 12 hours. The caller keeps this out
     * of streams (it belongs on the PC list); canShow says whether that screen is still in front when the answer arrives.
     */
    public static void autoCheck(Activity activity, BooleanSupplier canShow) {
        Context app = activity.getApplicationContext();
        if (!isAvailable(app) || updating.get()) {
            return;
        }
        UpdatePrefs prefs = new UpdatePrefs(app);
        long now = System.currentTimeMillis();
        if (!UpdatePolicy.isCheckDue(prefs.isAutoCheck(), prefs.lastCheck(), now) || !checking.compareAndSet(false, true)) {
            return;
        }
        new Thread(() -> {
            try {
                // Old downloads go once the app is running again, whether or not that install finished.
                UpdateDownloader.cleanUp(app);
                ReleaseInfo release = UpdateChecker.fetchLatest();
                prefs.markChecked(now);
                if (UpdatePolicy.shouldOffer(release, BuildConfig.VERSION_NAME, prefs.skippedVersion())) {
                    activity.runOnUiThread(() -> {
                        if (canShow.getAsBoolean() && !activity.isFinishing() && !activity.isDestroyed()) {
                            showOffer(activity, release);
                        }
                    });
                }
            } catch (UpdateException.RateLimited e) {
                // Asking again soon would only repeat it.
                prefs.markChecked(now);
            } catch (Exception e) {
                // Offline or GitHub is unreachable: stay silent and try again on the next start.
                LimeLog.info("Update check failed: " + e);
            } finally {
                checking.set(false);
            }
        }, "nightjar-update-check").start();
    }

    /** The Settings action: always asks, shows every newer release (even a skipped one) and says what happened. */
    public static void checkNow(Activity activity) {
        if (updating.get() || !checking.compareAndSet(false, true)) {
            return;
        }
        Toast.makeText(activity, R.string.update_checking, Toast.LENGTH_SHORT).show();
        UpdatePrefs prefs = new UpdatePrefs(activity);
        new Thread(() -> {
            int message = 0;
            ReleaseInfo found = null;
            try {
                ReleaseInfo release = UpdateChecker.fetchLatest();
                prefs.markChecked(System.currentTimeMillis());
                if (release.isUpdateFor(BuildConfig.VERSION_NAME)) {
                    found = release;
                } else {
                    message = R.string.update_up_to_date;
                }
            } catch (Exception e) {
                LimeLog.info("Update check failed: " + e);
                message = R.string.update_check_failed;
            } finally {
                checking.set(false);
            }
            ReleaseInfo offer = found;
            int toast = message;
            activity.runOnUiThread(() -> {
                if (activity.isFinishing() || activity.isDestroyed()) {
                    return;
                }
                if (offer != null) {
                    showOffer(activity, offer);
                } else {
                    Toast.makeText(activity, toast, Toast.LENGTH_SHORT).show();
                }
            });
        }, "nightjar-update-check").start();
    }

    static void showOffer(Activity activity, ReleaseInfo release) {
        String notes = ReleaseNotes.plain(release.body, NOTES_MAX_CHARS);
        String message = activity.getString(R.string.update_available_message, release.version(), BuildConfig.VERSION_NAME);
        if (!notes.isEmpty()) {
            message += "\n\n" + notes;
        }
        UiHelper.showDialog(activity, new NavigationDialogBuilder(activity)
                .setTitle(R.string.update_available_title)
                .setMessage(message)
                .setPositiveButton(R.string.update_action_update, (dialog, which) -> startUpdate(activity, release))
                .setNegativeButton(R.string.update_action_later, null)
                .setNeutralButton(R.string.update_action_skip, (dialog, which) ->
                        new UpdatePrefs(activity).skip(release.version()))
                .create());
    }

    private static void startUpdate(Activity activity, ReleaseInfo release) {
        Context app = activity.getApplicationContext();
        if (UpdateInstaller.needsInstallPermission(app)) {
            UiHelper.showDialog(activity, new NavigationDialogBuilder(activity)
                    .setTitle(R.string.update_allow_title)
                    .setMessage(R.string.update_allow_message)
                    .setPositiveButton(R.string.update_allow_open, (dialog, which) ->
                            activity.startActivity(UpdateInstaller.installPermissionSettings(app)))
                    .setNegativeButton(android.R.string.cancel, null)
                    .create());
            return;
        }
        if (!updating.compareAndSet(false, true)) {
            return;
        }

        UpdateDownloader downloader = new UpdateDownloader();
        LinearLayout content = new LinearLayout(activity);
        content.setOrientation(LinearLayout.VERTICAL);
        int padding = Math.round(24 * activity.getResources().getDisplayMetrics().density);
        content.setPadding(padding, padding / 2, padding, 0);
        TextView status = new TextView(activity);
        status.setText(R.string.update_downloading);
        LinearProgressIndicator bar = new LinearProgressIndicator(activity);
        bar.setMax(100);
        bar.setIndeterminate(true);
        content.addView(status);
        content.addView(bar);
        AlertDialog progress = new NavigationDialogBuilder(activity)
                .setTitle(activity.getString(R.string.update_downloading_title, release.version()))
                .setView(content)
                .setNegativeButton(android.R.string.cancel, (dialog, which) -> downloader.cancel())
                .setOnCancelListener(dialog -> downloader.cancel())
                .create();
        UiHelper.showDialog(activity, progress);

        new Thread(() -> {
            int[] lastPercent = {-1};
            try {
                File apk = downloader.download(app, release, BuildConfig.ROOT_BUILD, (done, total) -> {
                    int percent = total > 0 ? (int) Math.min(100, done * 100 / total) : -1;
                    if (percent == lastPercent[0]) {
                        return;
                    }
                    lastPercent[0] = percent;
                    activity.runOnUiThread(() -> {
                        if (!progress.isShowing()) {
                            return;
                        }
                        if (percent >= 0) {
                            bar.setIndeterminate(false);
                            bar.setProgressCompat(percent, true);
                            status.setText(app.getString(R.string.update_progress,
                                    Formatter.formatShortFileSize(app, done), Formatter.formatShortFileSize(app, total)));
                        }
                    });
                });
                activity.runOnUiThread(progress::dismiss);
                UpdateInstaller.install(app, apk);
            } catch (UpdateException e) {
                activity.runOnUiThread(progress::dismiss);
                if (e.reason != UpdateException.Reason.CANCELLED) {
                    LimeLog.warning("Update failed: " + e);
                    activity.runOnUiThread(() -> showError(activity, errorMessage(e.reason)));
                }
            } catch (Exception e) {
                LimeLog.warning("Update failed: " + e);
                activity.runOnUiThread(progress::dismiss);
                activity.runOnUiThread(() -> showError(activity, R.string.update_error_install_start));
            } finally {
                updating.set(false);
            }
        }, "nightjar-update").start();
    }

    private static int errorMessage(UpdateException.Reason reason) {
        switch (reason) {
            case NO_APK:
                return R.string.update_error_no_apk;
            case NO_CHECKSUMS:
                return R.string.update_error_no_checksums;
            case CHECKSUM_ENTRY_MISSING:
                return R.string.update_error_checksum_missing;
            case CHECKSUM_MISMATCH:
                return R.string.update_error_checksum_mismatch;
            default:
                return R.string.update_error_download;
        }
    }

    private static void showError(Activity activity, int message) {
        if (activity.isFinishing() || activity.isDestroyed()) {
            Toast.makeText(activity.getApplicationContext(), message, Toast.LENGTH_LONG).show();
            return;
        }
        UiHelper.showDialog(activity, new NavigationDialogBuilder(activity)
                .setTitle(R.string.update_error_title)
                .setMessage(message)
                .setPositiveButton(android.R.string.ok, null)
                .create());
    }
}

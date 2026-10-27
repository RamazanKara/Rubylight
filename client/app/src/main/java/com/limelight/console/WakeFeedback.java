package com.limelight.console;

import android.app.Activity;

import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.limelight.R;
import com.limelight.ui.NavigationDialogBuilder;

/** What the player is told when waking a PC does not work out: a dialog a controller can reach, with Try again. */
final class WakeFeedback {
    private WakeFeedback() {
    }

    /** The PC did not come online in time. */
    static void timedOut(Activity activity, String hostName, Runnable retry) {
        show(activity, hostName, activity.getString(R.string.console_wake_timeout, hostName), retry);
    }

    /** The wake packet could not be sent (no network), or there is no MAC address to send it to. */
    static void failed(Activity activity, String hostName, int message, Runnable retry) {
        show(activity, hostName, activity.getString(message), retry);
    }

    private static void show(Activity activity, String title, String message, Runnable retry) {
        if (activity.isFinishing() || activity.isDestroyed()) {
            return;
        }
        MaterialAlertDialogBuilder builder = new NavigationDialogBuilder(activity).setTitle(title).setMessage(message);
        if (retry == null) {
            builder.setPositiveButton(android.R.string.ok, null);
        } else {
            builder.setNegativeButton(android.R.string.cancel, null)
                    .setPositiveButton(R.string.pair_try_again, (dialog, which) -> retry.run());
        }
        builder.show();
    }
}

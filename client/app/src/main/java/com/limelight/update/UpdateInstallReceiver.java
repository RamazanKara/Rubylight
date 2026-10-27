package com.limelight.update;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageInstaller;
import android.os.Build;
import android.widget.Toast;

import com.limelight.LimeLog;
import com.limelight.R;

/** Receives the PackageInstaller result: shows Android's confirmation screen when it is needed. */
public class UpdateInstallReceiver extends BroadcastReceiver {
    @Override
    public void onReceive(Context context, Intent intent) {
        int status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE);
        switch (status) {
            case PackageInstaller.STATUS_PENDING_USER_ACTION:
                Intent confirm = Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU
                        ? intent.getParcelableExtra(Intent.EXTRA_INTENT, Intent.class)
                        : intent.getParcelableExtra(Intent.EXTRA_INTENT);
                if (confirm != null) {
                    try {
                        context.startActivity(confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
                    } catch (RuntimeException e) {
                        LimeLog.warning("Update confirmation could not open: " + e);
                        Toast.makeText(context, R.string.update_error_install_blocked, Toast.LENGTH_LONG).show();
                    }
                }
                break;
            case PackageInstaller.STATUS_SUCCESS:
                UpdateDownloader.cleanUp(context);
                break;
            case PackageInstaller.STATUS_FAILURE_ABORTED:
                // The user said no.
                UpdateDownloader.cleanUp(context);
                break;
            default:
                String message = intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE);
                LimeLog.warning("Update install failed: " + status + " " + message);
                UpdateDownloader.cleanUp(context);
                Toast.makeText(context, context.getString(R.string.update_error_install,
                        message == null ? String.valueOf(status) : message), Toast.LENGTH_LONG).show();
                break;
        }
    }
}

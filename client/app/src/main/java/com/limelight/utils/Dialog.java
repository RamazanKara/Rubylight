package com.limelight.utils;

import java.util.ArrayList;

import android.app.Activity;
import androidx.appcompat.app.AlertDialog;
import com.limelight.ui.NavigationDialogBuilder;
import android.content.DialogInterface;
import android.widget.Button;

import com.limelight.R;

public class Dialog implements Runnable {
    private final String title;
    private final String message;
    private final Activity activity;
    private final Runnable runOnDismiss;

    private AlertDialog alert;
    private boolean dismissed;

    private static final ArrayList<Dialog> rundownDialogs = new ArrayList<>();

    private Dialog(Activity activity, String title, String message, Runnable runOnDismiss)
    {
        this.activity = activity;
        this.title = title;
        this.message = message;
        this.runOnDismiss = runOnDismiss;
        synchronized (rundownDialogs) {
            rundownDialogs.add(this);
        }
    }

    public static void closeDialogs(Activity activity)
    {
        activity.runOnUiThread(() -> {
            synchronized (rundownDialogs) {
                java.util.Iterator<Dialog> iterator = rundownDialogs.iterator();
                while (iterator.hasNext()) {
                    Dialog d = iterator.next();
                    if (d.activity != activity) {
                        continue;
                    }
                    iterator.remove();
                    d.dismissed = true;
                    if (d.alert != null) {
                        d.alert.dismiss();
                    }
                }
            }
        });
    }

    public static void displayDialog(final Activity activity, String title, String message, final boolean endAfterDismiss)
    {
        activity.runOnUiThread(new Dialog(activity, title, message, new Runnable() {
            @Override
            public void run() {
                if (endAfterDismiss) {
                    activity.finish();
                }
            }
        }));
    }

    public static void displayDialog(Activity activity, String title, String message, Runnable runOnDismiss)
    {
        activity.runOnUiThread(new Dialog(activity, title, message, runOnDismiss));
    }

    @Override
    public void run() {
        // If we're dying, don't bother creating a dialog
        if (dismissed || activity.isFinishing() || activity.isDestroyed()) {
            synchronized (rundownDialogs) {
                rundownDialogs.remove(this);
            }
            return;
        }

        alert = new NavigationDialogBuilder(activity)
                .setTitle(title)
                .setMessage(message)
                .setCancelable(true)
                .setOnCancelListener(dialog -> complete(false))
                .setPositiveButton(android.R.string.ok, (dialog, which) -> complete(false))
                .setNeutralButton(R.string.help, (dialog, which) -> complete(true))
                .create();
        alert.setCanceledOnTouchOutside(false);
        alert.setOnShowListener(new DialogInterface.OnShowListener(){

            @Override
            public void onShow(DialogInterface dialog) {
                // Set focus to the OK button by default
                Button button = alert.getButton(AlertDialog.BUTTON_POSITIVE);
                button.setFocusable(true);
                button.setFocusableInTouchMode(true);
                button.requestFocus();
            }
        });

        synchronized (rundownDialogs) {
            alert.show();
        }
    }

    private void complete(boolean help) {
        synchronized (rundownDialogs) {
            if (dismissed) return;
            dismissed = true;
            rundownDialogs.remove(this);
        }
        alert.dismiss();
        runOnDismiss.run();
        if (help) HelpLauncher.showHelp(activity);
    }

}

package com.limelight.utils;

import java.util.ArrayList;
import java.util.Iterator;

import android.app.Activity;
import androidx.appcompat.app.AlertDialog;
import com.limelight.ui.NavigationDialogBuilder;
import com.google.android.material.progressindicator.CircularProgressIndicator;
import android.widget.LinearLayout;
import android.content.DialogInterface;
import android.content.DialogInterface.OnCancelListener;

public class SpinnerDialog implements Runnable,OnCancelListener {
    private final String title;
    private final String message;
    private final Activity activity;
    private AlertDialog progress;
    private final boolean finish;
    private volatile boolean dismissed;

    private static final ArrayList<SpinnerDialog> rundownDialogs = new ArrayList<>();

    private SpinnerDialog(Activity activity, String title, String message, boolean finish)
    {
        this.activity = activity;
        this.title = title;
        this.message = message;
        this.progress = null;
        this.finish = finish;
    }

    public static SpinnerDialog displayDialog(Activity activity, String title, String message, boolean finish)
    {
        SpinnerDialog spinner = new SpinnerDialog(activity, title, message, finish);
        synchronized (rundownDialogs) {
            rundownDialogs.add(spinner);
        }
        activity.runOnUiThread(spinner);
        return spinner;
    }

    public static void closeDialogs(Activity activity)
    {
        synchronized (rundownDialogs) {
            Iterator<SpinnerDialog> i = rundownDialogs.iterator();
            while (i.hasNext()) {
                SpinnerDialog dialog = i.next();
                if (dialog.activity == activity) {
                    i.remove();
                    dialog.dismissed = true;
                    if (dialog.progress != null && dialog.progress.isShowing()) {
                        dialog.progress.dismiss();
                    }
                }
            }
        }
    }

    public void dismiss()
    {
        dismissed = true;
        activity.runOnUiThread(() -> {
            synchronized (rundownDialogs) {
                rundownDialogs.remove(this);
            }
            if (progress != null) {
                progress.dismiss();
            }
        });
    }

    public void setMessage(final String message)
    {
        activity.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (progress != null && !dismissed) {
                    progress.setMessage(message);
                }
            }
        });
    }

    @Override
    public void run() {

        // If we're dying, don't bother doing anything
        if (dismissed || activity.isFinishing() || activity.isDestroyed()) {
            synchronized (rundownDialogs) {
                rundownDialogs.remove(this);
            }
            return;
        }

        if (progress == null)
        {
            int padding = Math.round(24 * activity.getResources().getDisplayMetrics().density);
            LinearLayout content = new LinearLayout(activity);
            content.setGravity(android.view.Gravity.CENTER);
            content.setPadding(padding, padding / 2, padding, padding);
            CircularProgressIndicator indicator = new CircularProgressIndicator(activity);
            indicator.setIndeterminate(true);
            indicator.setImportantForAccessibility(android.view.View.IMPORTANT_FOR_ACCESSIBILITY_NO);
            content.addView(indicator);
            progress = new NavigationDialogBuilder(activity)
                    .setTitle(title).setMessage(message).setView(content).create();
            progress.setOnCancelListener(this);

            // If we want to finish the activity when this is killed, make it cancellable
            if (finish)
            {
                progress.setCancelable(true);
                progress.setCanceledOnTouchOutside(false);
            }
            else
            {
                progress.setCancelable(false);
            }

            synchronized (rundownDialogs) {
                progress.show();
            }
        }
        else
        {
            synchronized (rundownDialogs) {
                if (rundownDialogs.remove(this) && progress.isShowing()) {
                    progress.dismiss();
                }
            }
        }
    }

    @Override
    public void onCancel(DialogInterface dialog) {
        dismissed = true;
        synchronized (rundownDialogs) {
            rundownDialogs.remove(this);
        }

        // This will only be called if finish was true, so we don't need to check again
        activity.finish();
    }
}

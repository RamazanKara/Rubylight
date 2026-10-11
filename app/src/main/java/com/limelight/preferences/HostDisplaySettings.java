package com.limelight.preferences;

import android.app.Activity;
import android.widget.Toast;
import androidx.appcompat.app.AlertDialog;

import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.limelight.R;

/** The "Display on the PC" picker for one PC; the choice is saved at once and used from the next stream. */
public final class HostDisplaySettings {
    private HostDisplaySettings() {
    }

    public static void show(Activity activity, String hostUuid, String hostName) {
        show(activity, hostUuid, hostName, null);
    }

    public static void show(Activity activity, String hostUuid, String hostName, Runnable onChanged) {
        if (hostUuid == null) {
            return;
        }
        HostDisplayChoice[] choices = HostDisplayChoice.values();
        CharSequence[] labels = new CharSequence[choices.length];
        for (int i = 0; i < choices.length; i++) {
            labels[i] = activity.getString(choices[i].label);
        }
        HostDisplayChoice current = HostDisplayChoice.load(activity, hostUuid);
        AlertDialog picker = new MaterialAlertDialogBuilder(activity)
                .setTitle(activity.getString(R.string.host_display_title, hostName))
                .setSingleChoiceItems(labels, current.ordinal(), (dialog, which) -> {
                    HostDisplayChoice.save(activity, hostUuid, choices[which]);
                    Toast.makeText(activity, R.string.host_display_saved, Toast.LENGTH_SHORT).show();
                    if (onChanged != null) onChanged.run();
                    dialog.dismiss();
                })
                .setNeutralButton(R.string.help, null)
                .setNegativeButton(android.R.string.cancel, null)
                .show();
        picker.getButton(AlertDialog.BUTTON_NEUTRAL).setOnClickListener(view ->
                new MaterialAlertDialogBuilder(activity).setTitle(R.string.host_display_menu)
                        .setMessage(R.string.host_display_explanation)
                        .setPositiveButton(android.R.string.ok, null).show());
    }
}

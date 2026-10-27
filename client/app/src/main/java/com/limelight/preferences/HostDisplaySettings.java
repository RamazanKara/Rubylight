package com.limelight.preferences;

import android.app.Activity;
import android.widget.Toast;
import androidx.appcompat.app.AlertDialog;

import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.R;
import com.limelight.utils.UiHelper;

/** The display-mode picker for one PC. Only Save commits a choice for the next stream. */
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
        AlertDialog picker = create(activity, hostUuid, hostName, onChanged);
        UiHelper.showDialog(activity, picker);
        bindHelp(activity, picker);
    }

    /** Create separately so the profile screen can restore an unconfirmed selection after rotation. */
    static AlertDialog create(Activity activity, String hostUuid, String hostName, Runnable onChanged) {
        // Presentation order is independent of stable stored enum names and wire values.
        HostDisplayChoice[] choices = {
                HostDisplayChoice.HOST_DEFAULT, HostDisplayChoice.EXCLUSIVE,
                HostDisplayChoice.EXTENDED, HostDisplayChoice.EXTENDED_PRIMARY,
                HostDisplayChoice.EXTENDED_ISOLATED, HostDisplayChoice.EXTENDED_PRIMARY_ISOLATED,
                HostDisplayChoice.PHYSICAL};
        CharSequence[] labels = new CharSequence[choices.length];
        for (int i = 0; i < choices.length; i++) {
            labels[i] = activity.getString(choices[i].label);
        }
        HostDisplayChoice current = HostDisplayChoice.load(activity, hostUuid);
        int checked = java.util.Arrays.asList(choices).indexOf(current);
        return new NavigationDialogBuilder(activity)
                .setTitle(activity.getString(R.string.host_display_title, hostName))
                .setSingleChoiceItems(labels, checked, null)
                .setPositiveButton(R.string.host_display_save, (dialog, which) -> {
                    // Read the view state because the pending selection may have been restored.
                    int selected = ((AlertDialog) dialog).getListView().getCheckedItemPosition();
                    if (selected < 0 || selected >= choices.length) return;
                    HostDisplayChoice.save(activity, hostUuid, choices[selected]);
                    Toast.makeText(activity, R.string.host_display_saved, Toast.LENGTH_SHORT).show();
                    if (onChanged != null) onChanged.run();
                })
                .setNeutralButton(R.string.help, null)
                .setNegativeButton(android.R.string.cancel, null)
                .create();
    }

    static void bindHelp(Activity activity, AlertDialog picker) {
        picker.getButton(AlertDialog.BUTTON_NEUTRAL).setOnClickListener(view ->
                UiHelper.showDialog(activity, new NavigationDialogBuilder(activity).setTitle(R.string.host_display_menu)
                        .setMessage(R.string.host_display_explanation)
                        .setPositiveButton(android.R.string.ok, null).create()));
    }
}

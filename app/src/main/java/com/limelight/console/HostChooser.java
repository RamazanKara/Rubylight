package com.limelight.console;

import android.app.Activity;

import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.limelight.R;
import com.limelight.utils.UiHelper;

import java.util.List;
import java.util.function.Consumer;

/** The question "Play on which PC?" for a game several PCs have: a plain list a D-pad and a controller can walk. */
final class HostChooser {
    private HostChooser() {
    }

    /** What each entry says: the PC's name, and for a PC that is away that it is asleep. */
    static CharSequence[] labels(Activity activity, List<HostChoice> choices) {
        CharSequence[] labels = new CharSequence[choices.size()];
        for (int i = 0; i < labels.length; i++) {
            HostChoice choice = choices.get(i);
            labels[i] = choice.online ? choice.hostName : OfflineLook.withLabel(activity, choice.hostName);
        }
        return labels;
    }

    static void show(Activity activity, List<HostChoice> choices, Consumer<HostChoice> chosen) {
        if (activity.isFinishing() || activity.isDestroyed()) {
            return;
        }
        UiHelper.showDialog(activity, new MaterialAlertDialogBuilder(activity)
                .setTitle(R.string.console_choose_pc)
                .setItems(labels(activity, choices), (dialog, which) -> chosen.accept(choices.get(which)))
                .setNegativeButton(android.R.string.cancel, null).create());
    }
}

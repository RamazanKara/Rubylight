package com.limelight.ui;

import android.content.Context;
import android.content.DialogInterface;
import android.content.res.Configuration;
import android.view.KeyEvent;
import android.widget.ListView;

import androidx.appcompat.app.AlertDialog;

import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.limelight.R;

/** Material dialogs with the same Back/B/Escape navigation as the app's action sheets. */
public final class NavigationDialogBuilder extends MaterialAlertDialogBuilder {
    private DialogInterface.OnKeyListener keyListener;

    public NavigationDialogBuilder(Context context) { super(context); }

    @Override
    public MaterialAlertDialogBuilder setOnKeyListener(DialogInterface.OnKeyListener listener) {
        keyListener = listener;
        return super.setOnKeyListener(listener);
    }

    @Override
    public AlertDialog create() {
        AppTheme.outlineOnBlack(getContext(), getBackground());
        AlertDialog dialog = super.create();
        // A TV's list dialogs (Presets, the PC menu's choices) marked the D-pad row with the platform's grey block; they
        // take the amber row outline the Settings lists use instead. Phones keep the platform selector.
        ListView list = dialog.getListView();
        if (list != null && isTelevision(getContext())) list.setSelector(R.drawable.list_focus_row);
        DialogInterface.OnKeyListener custom = keyListener;
        // One filter per dialog: a pad that sends DPAD keys and a hat must not move its focus twice. The
        // dialog is short-lived, so it has no focus-loss reset (the listener has no hook for it).
        DpadEchoFilter dpadEchoes = new DpadEchoFilter();
        dialog.setOnKeyListener((target, key, event) -> {
            if (dpadEchoes.drop(event)) return true;
            if (custom != null && custom.onKey(target, key, event)) return true;
            if (!event.hasNoModifiers() || (key != KeyEvent.KEYCODE_BUTTON_B && key != KeyEvent.KEYCODE_ESCAPE)) return false;
            // Use the normal Back path: cancel callbacks and non-cancelable dialogs retain their rules.
            if (event.getAction() == KeyEvent.ACTION_UP && !event.isCanceled()) dialog.onBackPressed();
            return true;
        });
        return dialog;
    }

    private static boolean isTelevision(Context context) {
        return (context.getResources().getConfiguration().uiMode & Configuration.UI_MODE_TYPE_MASK)
                == Configuration.UI_MODE_TYPE_TELEVISION;
    }
}

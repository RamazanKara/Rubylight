package com.limelight.ui;

import android.content.Context;
import android.content.DialogInterface;
import android.view.KeyEvent;

import androidx.appcompat.app.AlertDialog;

import com.google.android.material.dialog.MaterialAlertDialogBuilder;

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
}

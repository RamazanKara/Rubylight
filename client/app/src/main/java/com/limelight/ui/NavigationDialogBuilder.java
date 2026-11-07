package com.limelight.ui;

import android.content.Context;
import android.content.DialogInterface;
import android.view.KeyEvent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.Button;
import android.widget.ListView;

import androidx.appcompat.app.AlertDialog;

import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.limelight.R;

/**
 * The app's dialogs: the glass panel, one moon action, and the same Back/B/Escape navigation as the action sheets. All of them
 * come from here, so they are one look on every form.
 */
public final class NavigationDialogBuilder extends MaterialAlertDialogBuilder {
    private DialogInterface.OnKeyListener keyListener;

    public NavigationDialogBuilder(Context context) {
        super(context);
        setBackground(GlassPanelDrawable.dialog(getContext()));
    }

    @Override
    public MaterialAlertDialogBuilder setOnKeyListener(DialogInterface.OnKeyListener listener) {
        keyListener = listener;
        return super.setOnKeyListener(listener);
    }

    @Override
    public AlertDialog create() {
        AlertDialog dialog = super.create();
        // The platform's selector is a grey block, which is no focus language of ours.
        ListView list = dialog.getListView();
        if (list != null) list.setSelector(R.drawable.dialog_row_focus);
        if (dialog.getWindow() != null) dialog.getWindow().setDimAmount(GlassPanelDrawable.backdropDim(getContext()));
        // Builds the views now: the buttons exist, and their bar can be laid out for capsules.
        dialog.create();
        styleButtons(dialog);
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

    private void styleButtons(AlertDialog dialog) {
        Button positive = dialog.getButton(DialogInterface.BUTTON_POSITIVE);
        if (positive == null || !(positive.getParent() instanceof ViewGroup)) return;
        // Capsules want air the text buttons did not: the bar stands on the panel's own margin.
        android.content.res.Resources res = getContext().getResources();
        int edge = res.getDimensionPixelSize(R.dimen.space_l);
        ViewGroup bar = (ViewGroup) positive.getParent();
        bar.setPaddingRelative(edge, res.getDimensionPixelSize(R.dimen.space_m), edge, edge);
        // The bar's scroller took the window's first focus and showed the platform's grey highlight over the whole bar.
        if (bar.getParent() instanceof android.widget.ScrollView) {
            View scroller = (View) bar.getParent();
            scroller.setFocusable(false);
            if (android.os.Build.VERSION.SDK_INT >= 26) scroller.setDefaultFocusHighlightEnabled(false);
        }
        // A pad or a remote starts on the moon, the dialog's one way on. A list keeps its first focus on its rows.
        if (android.os.Build.VERSION.SDK_INT >= 26 && dialog.getListView() == null) positive.setFocusedByDefault(true);
        // Labels too long to share a row stack: as full-width capsules, not a staircase along the right edge.
        bar.addOnLayoutChangeListener((view, l, t, r, b, ol, ot, or, ob) -> {
            if (bar instanceof android.widget.LinearLayout
                    && ((android.widget.LinearLayout) bar).getOrientation() == android.widget.LinearLayout.VERTICAL) {
                bar.post(() -> stretchStacked(bar));
            }
        });
    }

    private void stretchStacked(ViewGroup bar) {
        int gap = getContext().getResources().getDimensionPixelSize(R.dimen.space_s);
        boolean first = true;
        for (int i = 0; i < bar.getChildCount(); i++) {
            View child = bar.getChildAt(i);
            if (!(child instanceof Button) || child.getVisibility() != View.VISIBLE) continue;
            ViewGroup.MarginLayoutParams params = (ViewGroup.MarginLayoutParams) child.getLayoutParams();
            int top = first ? 0 : gap;
            first = false;
            if (params.width == ViewGroup.LayoutParams.MATCH_PARENT && params.topMargin == top && params.getMarginStart() == 0) continue;
            params.width = ViewGroup.LayoutParams.MATCH_PARENT;
            params.topMargin = top;
            params.setMarginStart(0);
            child.setLayoutParams(params);
        }
    }
}

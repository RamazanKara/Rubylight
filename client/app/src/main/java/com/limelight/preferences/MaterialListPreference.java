package com.limelight.preferences;

import android.content.Context;
import android.os.Bundle;
import android.preference.ListPreference;
import android.util.AttributeSet;
import androidx.appcompat.app.AlertDialog;

import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.R;
import com.limelight.utils.UiHelper;

public class MaterialListPreference extends ListPreference {
    public MaterialListPreference(Context context, AttributeSet attrs) {
        super(context, attrs);
        // The row shows the chosen value; the explanation opens from the picker's Help button.
        if (getDialogMessage() == null) {
            setDialogMessage(getSummary());
        }
    }

    @Override
    protected void showDialog(Bundle state) {
        MaterialAlertDialogBuilder builder = new NavigationDialogBuilder(getContext())
                .setTitle(getTitle())
                .setSingleChoiceItems(getEntries(), findIndexOfValue(getValue()), (dialog, which) -> {
                    String value = getEntryValues()[which].toString();
                    if (callChangeListener(value)) {
                        setValue(value);
                    }
                    dialog.dismiss();
                })
                .setNegativeButton(android.R.string.cancel, null);
        if (getDialogMessage() != null) {
            builder.setNeutralButton(R.string.help, null);
        }
        customizePicker(builder);
        AlertDialog picker = builder.create();
        UiHelper.showDialog(getContext(), picker);
        if (getDialogMessage() != null) {
            picker.getButton(AlertDialog.BUTTON_NEUTRAL).setOnClickListener(view ->
                    UiHelper.showDialog(getContext(), new NavigationDialogBuilder(getContext()).setTitle(getTitle())
                            .setMessage(getDialogMessage()).setPositiveButton(android.R.string.ok, null).create()));
        }
    }

    protected void customizePicker(MaterialAlertDialogBuilder builder) {
    }
}

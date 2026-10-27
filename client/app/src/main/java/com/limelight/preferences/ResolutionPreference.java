package com.limelight.preferences;

import android.content.Context;
import android.text.InputType;
import android.util.AttributeSet;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.ScrollView;

import androidx.appcompat.app.AlertDialog;

import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.limelight.ui.NavigationDialogBuilder;
import com.google.android.material.textfield.TextInputEditText;
import com.google.android.material.textfield.TextInputLayout;
import com.limelight.R;
import com.limelight.utils.UiHelper;

import java.util.Arrays;

/** Global stream dimensions, including fixed sizes that are not display modes or presets. */
public class ResolutionPreference extends MaterialListPreference {
    private boolean choosingCustomResolution;

    public ResolutionPreference(Context context, AttributeSet attrs) {
        super(context, attrs);
    }

    boolean isChoosingCustomResolution() {
        return choosingCustomResolution;
    }

    // Keep a saved custom size visible after the device-dependent preset list has been built.
    void ensureSelectedResolutionEntry() {
        ensureResolutionEntry(getValue());
    }

    private void ensureResolutionEntry(String value) {
        if (value == null || findIndexOfValue(value) >= 0) return;
        CharSequence[] names = Arrays.copyOf(getEntries(), getEntries().length + 1);
        CharSequence[] values = Arrays.copyOf(getEntryValues(), getEntryValues().length + 1);
        names[names.length - 1] = getContext().getString(R.string.resolution_custom_size,
                value.replace('x', '×'));
        values[values.length - 1] = value;
        setEntries(names);
        setEntryValues(values);
    }

    @Override
    protected void customizePicker(MaterialAlertDialogBuilder builder) {
        builder.setPositiveButton(R.string.resolution_custom, (dialog, which) -> showCustomResolution());
    }

    private void showCustomResolution() {
        Context context = getContext();
        LinearLayout form = new LinearLayout(context);
        form.setOrientation(LinearLayout.VERTICAL);
        int padding = Math.round(24 * context.getResources().getDisplayMetrics().density);
        form.setPadding(padding, 0, padding, padding);
        String value = getValue();
        TextInputLayout width = field(form, R.id.resolution_width, R.string.host_profile_width,
                PreferenceConfiguration.getWidthFromResolutionString(value));
        TextInputLayout height = field(form, R.id.resolution_height, R.string.host_profile_height,
                PreferenceConfiguration.getHeightFromResolutionString(value));
        ScrollView scroll = new ScrollView(context);
        scroll.addView(form);
        AlertDialog editor = new NavigationDialogBuilder(context).setTitle(R.string.title_resolution_list)
                .setView(scroll).setPositiveButton(android.R.string.ok, null)
                .setNegativeButton(android.R.string.cancel, null).create();
        UiHelper.showDialog(context, editor);
        editor.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(view -> {
            Integer w = readDimension(width);
            Integer h = readDimension(height);
            if (w == null || h == null) return;
            String resolution = w + "x" + h;
            choosingCustomResolution = true;
            try {
                if (!callChangeListener(resolution)) return;
                ensureResolutionEntry(resolution);
                setValue(resolution);
            } finally {
                choosingCustomResolution = false;
            }
            editor.dismiss();
        });
    }

    private TextInputLayout field(LinearLayout form, int id, int label, int value) {
        TextInputLayout container = new TextInputLayout(getContext());
        container.setHint(label);
        EditText input = new TextInputEditText(container.getContext());
        input.setId(id);
        input.setContentDescription(getContext().getString(label));
        input.setInputType(InputType.TYPE_CLASS_NUMBER);
        input.setSingleLine(true);
        input.setSelectAllOnFocus(true);
        input.setMinHeight(Math.round(56 * getContext().getResources().getDisplayMetrics().density));
        input.setText(Integer.toString(value));
        container.addView(input);
        form.addView(container);
        return container;
    }

    private Integer readDimension(TextInputLayout container) {
        container.setError(null);
        try {
            return HostStreamProfile.requireRange(Integer.parseInt(
                    container.getEditText().getText().toString().trim()), 64, 16384);
        } catch (IllegalArgumentException e) {
            container.setError(getContext().getString(R.string.host_profile_invalid_number, 64, 16384));
            return null;
        }
    }
}

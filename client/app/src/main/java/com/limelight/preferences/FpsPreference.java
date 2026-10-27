package com.limelight.preferences;

import android.content.Context;
import android.content.SharedPreferences;
import android.preference.PreferenceManager;
import android.util.AttributeSet;

import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.limelight.R;

/**
 * The frame rate list. The list offers only the rates this screen supports unless {@code checkbox_unlock_fps} is
 * on, so the picker itself switches between that list and every rate, instead of a switch elsewhere in Settings.
 */
public class FpsPreference extends MaterialListPreference {
    public FpsPreference(Context context, AttributeSet attrs) {
        super(context, attrs);
    }

    @Override
    protected void customizePicker(MaterialAlertDialogBuilder builder) {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(getContext());
        boolean all = prefs.getBoolean(PreferenceConfiguration.UNLOCK_FPS_STRING, false);
        builder.setPositiveButton(all ? R.string.fps_show_supported : R.string.title_unlock_fps, (dialog, which) -> {
            prefs.edit().putBoolean(PreferenceConfiguration.UNLOCK_FPS_STRING, !all).apply();
            // The list is built from this choice when the screen is created.
            if (getContext() instanceof StreamSettings) ((StreamSettings) getContext()).reloadSettings();
        });
    }
}

package com.limelight.binding.audio;

import android.content.Context;

import com.limelight.R;

import java.util.List;

/**
 * What the stream says when the microphone settings ask for processing this device does not have:
 * the effects by their setting names, so the user knows which switch has no effect here.
 */
public final class MicNotice {
    private MicNotice() { }

    /** The setting that turns {@code effect} on, as its title string. */
    public static int title(MicProcessing.Effect effect) {
        switch (effect) {
            case ECHO_CANCELER: return R.string.title_list_mic_echo_cancel;
            case NOISE_SUPPRESSOR: return R.string.title_checkbox_mic_noise_suppression;
            default: return R.string.title_checkbox_mic_auto_gain;
        }
    }

    /** The in-stream notice for {@code missing}, or null when nothing is missing. */
    public static String text(Context context, List<MicProcessing.Effect> missing) {
        if (missing == null || missing.isEmpty()) return null;
        StringBuilder names = new StringBuilder();
        for (MicProcessing.Effect effect : missing) {
            if (names.length() > 0) names.append(", ");
            names.append(context.getString(title(effect)));
        }
        return context.getString(R.string.mic_effects_missing, names);
    }
}

package com.limelight.preferences;

import android.content.Context;
import android.util.AttributeSet;

/**
 * Auto, On or Off for the HEVC low-latency flags, the choice Moonlight V+ offers as "HEVC
 * low-latency decoding" (qiin2333/moonlight-vplus, GPL-3.0). It replaces the older on/off switch:
 * Off is still stored as that switch's "disable" boolean, so older saved values and profiles show
 * Off, and the list's own key only tells Auto from On (see PreferenceConfiguration).
 */
public class HevcLowLatencyPreference extends MaterialListPreference {
    public HevcLowLatencyPreference(Context context, AttributeSet attrs) {
        super(context, attrs);
    }

    // The list's own key is missing on installs from before it existed; applying the "auto" default
    // there would overwrite an Off stored in the older key, so the value always comes from storage.
    @Override
    protected void onSetInitialValue(boolean restoreValue, Object defaultValue) {
        super.onSetInitialValue(true, defaultValue);
    }

    // ListPreference reads and writes its value through these two, so both keys stay in step.
    @Override
    protected String getPersistedString(String defaultReturnValue) {
        if (!shouldPersist()) return defaultReturnValue;
        return PreferenceConfiguration.hevcLowLatencyModeValue(
                PreferenceConfiguration.readHevcLowLatencyMode(getSharedPreferences()));
    }

    @Override
    protected boolean persistString(String value) {
        if (!shouldPersist()) return false;
        int mode = "on".equals(value) ? PreferenceConfiguration.HEVC_LOW_LATENCY_ON :
                "off".equals(value) ? PreferenceConfiguration.HEVC_LOW_LATENCY_OFF :
                        PreferenceConfiguration.HEVC_LOW_LATENCY_AUTO;
        android.content.SharedPreferences.Editor editor = getSharedPreferences().edit();
        PreferenceConfiguration.writeHevcLowLatencyMode(editor, mode);
        editor.apply();
        return true;
    }
}

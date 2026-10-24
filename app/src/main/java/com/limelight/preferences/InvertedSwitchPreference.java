package com.limelight.preferences;

import android.content.Context;
import android.util.AttributeSet;

/**
 * A switch that shows the opposite of the stored boolean. A setting that is stored as "disable X" can then read
 * "use X" on screen while old saved values, profiles and the stream configuration keep the same key and meaning.
 */
public class InvertedSwitchPreference extends MaterialSwitchPreference {
    public InvertedSwitchPreference(Context context, AttributeSet attrs) {
        super(context, attrs);
    }

    // The framework's persistBoolean compares against getPersistedBoolean, so both are written out here.
    @Override
    protected boolean getPersistedBoolean(boolean defaultReturnValue) {
        if (!shouldPersist()) return defaultReturnValue;
        return !getSharedPreferences().getBoolean(getKey(), !defaultReturnValue);
    }

    @Override
    protected boolean persistBoolean(boolean value) {
        if (!shouldPersist()) return false;
        getSharedPreferences().edit().putBoolean(getKey(), !value).apply();
        return true;
    }
}

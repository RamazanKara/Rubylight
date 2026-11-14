package com.limelight.preferences;

import android.content.Context;
import android.content.SharedPreferences;
import android.util.AttributeSet;

/**
 * The Variable refresh rate row: Off, On or Fastest, drawn as a list but stored in the two switches
 * {@code checkbox_vrr} and {@code checkbox_real_vrr} (VrrChoice), never as a string under its key. The value is
 * read from those switches each time, so presets, profiles and other writers show at once.
 */
public class VrrPreference extends MaterialListPreference {
    public VrrPreference(Context context, AttributeSet attrs) {
        super(context, attrs);
    }

    @Override
    protected String getPersistedString(String defaultReturnValue) {
        SharedPreferences prefs = getSharedPreferences();
        if (!shouldPersist() || prefs == null) return defaultReturnValue;
        return VrrChoice.VALUES[VrrChoice.read(prefs)];
    }

    @Override
    protected boolean persistString(String value) {
        SharedPreferences prefs = getSharedPreferences();
        if (!shouldPersist() || prefs == null) return false;
        int choice = VrrChoice.ofValue(value);
        // A switch first stored its default too; an unchanged choice writes nothing else.
        if (choice == VrrChoice.read(prefs) && prefs.contains(getKey())) return true;
        SharedPreferences.Editor editor = prefs.edit();
        VrrChoice.save(editor, choice);
        editor.apply();
        return true;
    }

    @Override
    public void setValue(String value) {
        super.setValue(value);
        // The list remembers its last value; the switches may have changed since without it.
        persistString(value);
    }

    @Override
    public String getValue() {
        SharedPreferences prefs = getSharedPreferences();
        return shouldPersist() && prefs != null ? VrrChoice.VALUES[VrrChoice.read(prefs)] : super.getValue();
    }

    @Override
    public CharSequence getEntry() {
        int index = findIndexOfValue(getValue());
        CharSequence[] entries = getEntries();
        return index >= 0 && entries != null ? entries[index] : null;
    }
}

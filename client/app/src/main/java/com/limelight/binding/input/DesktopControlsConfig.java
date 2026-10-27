package com.limelight.binding.input;

import android.content.Context;
import android.preference.PreferenceManager;

import org.json.JSONException;
import org.json.JSONObject;

/** Independent, bounded preferences for the desktop panel; never changes the stream's touch mode. */
public final class DesktopControlsConfig {
    private static final String PREF = "desktop_controls_config_v1";
    public String mode = "keyboard";
    public String bank = "typing";
    public int keyHeightDp = 40;
    public int panelHeightDp = 300;
    public boolean panelHeightCustomized;
    public int opacityPercent = 96;
    public int pointerSpeedPercent = 100;
    public boolean leftHanded;
    public boolean naturalScroll = true;
    public boolean tapToClick = true;
    public boolean mouseButtons = true;
    public boolean middleButton;
    public boolean extraButtons;
    public boolean lockModifiers;
    public boolean functionBank = true;
    public boolean navigationBank = true;
    public boolean numpadBank = true;
    public boolean shortcutsBank = true;

    public static DesktopControlsConfig load(Context context) {
        try {
            return parse(PreferenceManager.getDefaultSharedPreferences(context).getString(PREF, "{}"));
        } catch (ClassCastException e) {
            return new DesktopControlsConfig();
        }
    }

    public void save(Context context) {
        PreferenceManager.getDefaultSharedPreferences(context).edit().putString(PREF, encode()).apply();
    }

    public DesktopControlsConfig copy() { return parse(encode()); }

    public static DesktopControlsConfig parse(String json) {
        DesktopControlsConfig value = new DesktopControlsConfig();
        if (json == null || json.length() > 8192) return value;
        try {
            JSONObject object = new JSONObject(json);
            if (object.optInt("version", 1) != 1) return value;
            value.mode = choice(object.optString("mode"), "keyboard", "trackpad", "combined");
            value.bank = choice(object.optString("bank"), "typing", "function", "navigation", "numpad", "shortcuts");
            value.keyHeightDp = clamp(object.optInt("keyHeightDp", 40), 36, 56);
            value.panelHeightDp = clamp(object.optInt("panelHeightDp", 300), 220, 560);
            value.panelHeightCustomized = object.optBoolean("panelHeightCustomized", false);
            value.opacityPercent = clamp(object.optInt("opacityPercent", 96), 50, 100);
            value.pointerSpeedPercent = clamp(object.optInt("pointerSpeedPercent", 100), 25, 400);
            value.leftHanded = object.optBoolean("leftHanded", false);
            value.naturalScroll = object.optBoolean("naturalScroll", true);
            value.tapToClick = object.optBoolean("tapToClick", true);
            value.mouseButtons = object.optBoolean("mouseButtons", true);
            value.middleButton = object.optBoolean("middleButton", false);
            value.extraButtons = object.optBoolean("extraButtons", false);
            value.lockModifiers = object.optBoolean("lockModifiers", false);
            value.functionBank = object.optBoolean("functionBank", true);
            value.navigationBank = object.optBoolean("navigationBank", true);
            value.numpadBank = object.optBoolean("numpadBank", true);
            value.shortcutsBank = object.optBoolean("shortcutsBank", true);
        } catch (JSONException ignored) {
            return new DesktopControlsConfig();
        }
        return value;
    }

    public String encode() {
        try {
            return new JSONObject().put("version", 1).put("mode", mode).put("bank", bank)
                    .put("keyHeightDp", keyHeightDp).put("panelHeightDp", panelHeightDp)
                    .put("panelHeightCustomized", panelHeightCustomized)
                    .put("opacityPercent", opacityPercent).put("pointerSpeedPercent", pointerSpeedPercent)
                    .put("leftHanded", leftHanded).put("naturalScroll", naturalScroll)
                    .put("tapToClick", tapToClick).put("mouseButtons", mouseButtons)
                    .put("middleButton", middleButton).put("extraButtons", extraButtons)
                    .put("lockModifiers", lockModifiers).put("functionBank", functionBank)
                    .put("navigationBank", navigationBank).put("numpadBank", numpadBank)
                    .put("shortcutsBank", shortcutsBank).toString();
        } catch (JSONException impossible) {
            throw new IllegalStateException(impossible);
        }
    }

    private static String choice(String value, String first, String... others) {
        if (first.equals(value)) return first;
        for (String other : others) if (other.equals(value)) return other;
        return first;
    }

    private static int clamp(int value, int minimum, int maximum) {
        return Math.max(minimum, Math.min(maximum, value));
    }
}

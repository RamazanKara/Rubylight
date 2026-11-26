/** The "HDR format" setting's values and which of them this screen is offered. */
package com.limelight.preferences;

import android.app.Activity;

import com.limelight.binding.video.HdrPolicy;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

/** The order is nightjar_policy::video_codec::hdr::HdrFormatChoice's, passed by ordinal through JNI. */
public enum HdrFormatChoice {
    AUTO("auto"),
    NO_DOLBY_VISION("no_dolby_vision"),
    HDR10("hdr10");

    public static final String PREF_KEY = "list_hdr_format";

    /** The stored value. */
    public final String value;

    HdrFormatChoice(String value) {
        this.value = value;
    }

    /** A stored value; anything unknown is Automatic. */
    public static HdrFormatChoice fromPreference(String stored) {
        for (HdrFormatChoice choice : values()) {
            if (choice.value.equals(stored)) return choice;
        }
        return AUTO;
    }

    static HdrFormatChoice require(String stored) {
        HdrFormatChoice choice = fromPreference(stored);
        if (!choice.value.equals(stored)) throw new IllegalArgumentException("Invalid HDR format");
        return choice;
    }

    /**
     * No Dolby Vision is left out on a screen that reports none (nothing asks for it there), unless
     * it is the selected one: a stored choice stays listed.
     */
    static List<HdrFormatChoice> offered(boolean dolbyVisionScreen, HdrFormatChoice selected) {
        List<HdrFormatChoice> offered = new ArrayList<>(Arrays.asList(values()));
        if (!dolbyVisionScreen && selected != NO_DOLBY_VISION) offered.remove(NO_DOLBY_VISION);
        return offered;
    }

    @SuppressWarnings("deprecation")
    static List<HdrFormatChoice> offered(Activity activity, HdrFormatChoice selected) {
        return offered(HdrPolicy.hasDolbyVision(
                HdrPolicy.displayHdrTypes(activity.getWindowManager().getDefaultDisplay())), selected);
    }
}

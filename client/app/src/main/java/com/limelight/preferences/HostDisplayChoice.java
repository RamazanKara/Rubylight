package com.limelight.preferences;

import android.content.Context;
import android.content.SharedPreferences;

import com.limelight.R;
import com.limelight.nvstream.http.NvApp;

/**
 * What a PC's displays do while this device streams from it, chosen per PC and sent as the
 * {@code hostDisplay} launch and resume parameter. Rubylight hosts honour it for that stream;
 * other hosts ignore it, so the virtual choices also send {@code virtualDisplay=1}.
 */
public enum HostDisplayChoice {
    /** The host's own display settings decide; nothing is sent. */
    HOST_DEFAULT(null, R.string.host_display_default),
    /** A virtual display, with the PC's other displays off for the stream. */
    EXCLUSIVE("exclusive", R.string.host_display_exclusive),
    /** A virtual display beside the PC's displays, made primary so games open on it. */
    EXTENDED_PRIMARY("extended_primary", R.string.host_display_extended_primary),
    /** A virtual display beside the PC's displays; the PC keeps its primary display. */
    EXTENDED("extended", R.string.host_display_extended),
    /** Keep the PC primary and place the virtual display away from its other displays. */
    EXTENDED_ISOLATED("extended_isolated", R.string.host_display_extended_isolated),
    /** Make the virtual display primary and place the PC's other displays away from it. */
    EXTENDED_PRIMARY_ISOLATED("extended_primary_isolated", R.string.host_display_extended_primary_isolated),
    /** No virtual display: stream the PC's physical display. */
    PHYSICAL("physical", R.string.host_display_physical);

    public static final String LAUNCH_PARAMETER = "hostDisplay";
    private static final String PREFERENCES = "HostDisplayChoices";

    /** The {@code hostDisplay} value, or null to send nothing. */
    public final String wireValue;
    public final int label;

    HostDisplayChoice(String wireValue, int label) {
        this.wireValue = wireValue;
        this.label = label;
    }

    /** Only a stream chooses; Remote Monitor and Remote Input keep the host's settings. */
    public HostDisplayChoice forRole(NvApp.Role role) {
        return role == NvApp.Role.STREAM ? this : HOST_DEFAULT;
    }

    /**
     * Whether the launch asks for a virtual display ({@code virtualDisplay=1}): the virtual
     * choices do, the physical choice does not, and the host default keeps the
     * "Host virtual display" setting.
     */
    public boolean requestsVirtualDisplay(boolean virtualDisplaySetting) {
        switch (this) {
            case HOST_DEFAULT:
                return virtualDisplaySetting;
            case PHYSICAL:
                return false;
            default:
                return true;
        }
    }

    /** A stored name; anything unknown (a newer or damaged value) is the host default. */
    static HostDisplayChoice fromStored(String name) {
        if (name != null) {
            for (HostDisplayChoice choice : values()) {
                if (choice.name().equals(name)) {
                    return choice;
                }
            }
        }
        return HOST_DEFAULT;
    }

    public static HostDisplayChoice load(Context context, String hostUuid) {
        if (context == null || hostUuid == null) {
            return HOST_DEFAULT;
        }
        return fromStored(context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
                .getString(hostUuid, null));
    }

    public static void save(Context context, String hostUuid, HostDisplayChoice choice) {
        if (hostUuid == null) {
            return;
        }
        SharedPreferences.Editor editor = context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE).edit();
        if (choice == HOST_DEFAULT) {
            editor.remove(hostUuid);
        } else {
            editor.putString(hostUuid, choice.name());
        }
        editor.apply();
    }
}

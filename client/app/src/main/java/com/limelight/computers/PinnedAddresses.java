package com.limelight.computers;

import android.content.Context;
import android.content.SharedPreferences;

import com.limelight.LimeLog;
import com.limelight.nvstream.http.ComputerDetails;

/**
 * The address each PC is pinned to, kept on this device only beside the database, as {@link HostNames}
 * keeps aliases. A pinned PC is polled and streamed to only at that address (Moonlight V+ PC's address
 * pinning); none means the automatic pick. Removing a PC forgets its pin.
 */
public final class PinnedAddresses {
    static final String PREFERENCES = "PinnedAddresses";

    private PinnedAddresses() {
    }

    private static SharedPreferences preferences(Context context) {
        return context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE);
    }

    /** The address this PC is pinned to, or null for the automatic pick. */
    public static ComputerDetails.AddressTuple get(Context context, String hostUuid) {
        if (context == null || hostUuid == null) {
            return null;
        }
        String stored = preferences(context).getString(hostUuid, null);
        int split = stored == null ? -1 : stored.lastIndexOf(' ');
        if (split <= 0) {
            return null;
        }
        try {
            return new ComputerDetails.AddressTuple(stored.substring(0, split), Integer.parseInt(stored.substring(split + 1)));
        } catch (IllegalArgumentException e) {
            LimeLog.warning("Pinned address unreadable, connecting automatically: " + e.getMessage());
            return null;
        }
    }

    /** Pins the PC to {@code address}, or brings back the automatic pick when it is null. */
    public static synchronized void set(Context context, String hostUuid, ComputerDetails.AddressTuple address) {
        if (context == null || hostUuid == null) {
            return;
        }
        SharedPreferences.Editor editor = preferences(context).edit();
        if (address == null) {
            editor.remove(hostUuid);
        } else {
            editor.putString(hostUuid, address.address + " " + address.port);
        }
        editor.apply();
    }
}
